// SPDX-License-Identifier: Apache-2.0
use futures_util::StreamExt;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

use super::skill_fs::SecureDir;
use crate::AppState;

// ── Data structures ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: String,
    pub tags: Vec<String>,
    pub enabled: bool,
    pub path: String,
    pub source: String,
    /// Transitional Phase 0 projection. `corrupt` remains visible in the
    /// resource center but can never be enabled or loaded.
    pub lifecycle_status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub template: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDetail {
    pub manifest: SkillManifest,
    pub system_prompt: String,
    pub slash_commands: Vec<SlashCommand>,
    pub has_tool_policy: bool,
    pub tool_policy: Option<String>,
    /// Digest of the exact metadata and bytes rendered by this detail view.
    /// Enabling uses it as a compare-and-swap precondition.
    pub review_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillSourceSelection {
    pub source_handle: String,
    pub display_path: String,
}

struct SkillSourceGrant {
    directory: SecureDir,
    expires_at_ms: u128,
}

const SKILL_SOURCE_GRANT_TTL_MS: u128 = 10 * 60 * 1000;
const MAX_SKILL_SOURCE_GRANTS: usize = 64;
static SKILL_SOURCE_GRANTS: Lazy<Mutex<HashMap<String, SkillSourceGrant>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

// ── Manifest file on disk (without path/source which we add ourselves) ────────

#[derive(Debug, Deserialize)]
struct ManifestFile {
    id: String,
    name: String,
    description: String,
    version: String,
    author: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    enabled: bool,
    /// Optional embedded system_prompt for single-file installs
    #[serde(default)]
    system_prompt: Option<String>,
}

// ── Directory helpers ─────────────────────────────────────────────────────────

fn builtin_skills_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .resource_dir()
        .ok()
        .map(|r| r.join("resources").join("skills"))
}

fn user_skills_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("CodeFactory")
        .join("skills")
}

const ACTIVATION_REVIEW_SUFFIX: &str = ".review-v1";

fn activation_reviews_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("CodeFactory")
        .join("skill-activation-reviews")
}

fn activation_review_filename(id: &str) -> Result<String, String> {
    validate_skill_id(id)?;
    Ok(format!("{id}{ACTIVATION_REVIEW_SUFFIX}"))
}

fn now_epoch_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn register_skill_source(directory: SecureDir) -> Result<SkillSourceSelection, String> {
    let display_path = directory.path().to_string_lossy().to_string();
    let source_handle = format!("skill-source-{}", uuid::Uuid::new_v4());
    let now = now_epoch_ms();
    let mut grants = SKILL_SOURCE_GRANTS.lock().map_err(|_| {
        "SKILL_SOURCE_HANDLE_STATE_FAILED: source grants are unavailable".to_string()
    })?;
    grants.retain(|_, grant| grant.expires_at_ms > now);
    if grants.len() >= MAX_SKILL_SOURCE_GRANTS {
        if let Some(oldest) = grants
            .iter()
            .min_by_key(|(_, grant)| grant.expires_at_ms)
            .map(|(id, _)| id.clone())
        {
            grants.remove(&oldest);
        }
    }
    grants.insert(
        source_handle.clone(),
        SkillSourceGrant {
            directory,
            expires_at_ms: now + SKILL_SOURCE_GRANT_TTL_MS,
        },
    );
    Ok(SkillSourceSelection {
        source_handle,
        display_path,
    })
}

fn consume_skill_source(source_handle: &str) -> Result<SecureDir, String> {
    if !source_handle.starts_with("skill-source-") {
        return Err(
            "SKILL_SOURCE_HANDLE_REQUIRED: choose the directory in CodeFactory before importing"
                .to_string(),
        );
    }
    let now = now_epoch_ms();
    let mut grants = SKILL_SOURCE_GRANTS.lock().map_err(|_| {
        "SKILL_SOURCE_HANDLE_STATE_FAILED: source grants are unavailable".to_string()
    })?;
    let grant = grants.remove(source_handle).ok_or_else(|| {
        "SKILL_SOURCE_HANDLE_INVALID: the directory selection expired or was already used"
            .to_string()
    })?;
    if grant.expires_at_ms <= now {
        return Err("SKILL_SOURCE_HANDLE_EXPIRED: choose the directory again".to_string());
    }
    Ok(grant.directory)
}

#[tauri::command]
pub async fn select_skill_source_directory(
    app: AppHandle,
) -> Result<Option<SkillSourceSelection>, String> {
    let directory =
        tauri::async_runtime::spawn_blocking(move || -> Result<Option<SecureDir>, String> {
            let picked = app
                .dialog()
                .file()
                .set_title("选择 Skill 目录（含 SKILL.md 或 manifest.json，可整个仓库）")
                .blocking_pick_folder();
            let Some(path) = picked else {
                return Ok(None);
            };
            let path = path
                .into_path()
                .map_err(|error| format!("SKILL_SOURCE_DIALOG_PATH_INVALID: {error}"))?;
            SecureDir::open_existing(&path)
                .map(Some)
                .map_err(|error| format!("SKILL_SOURCE_NOT_DIRECTORY: {error}"))
        })
        .await
        .map_err(|error| format!("SKILL_SOURCE_DIALOG_FAILED: {error}"))??;
    let Some(directory) = directory else {
        return Ok(None);
    };
    register_skill_source(directory).map(Some)
}

// ── Scan a directory for skill manifests ──────────────────────────────────────

fn scan_skill_dir(dir: &PathBuf, source: &str) -> Vec<SkillManifest> {
    let Ok(root) = SecureDir::open_existing(dir) else {
        return vec![];
    };
    let reviews = if source == "user" {
        SecureDir::open_existing(&activation_reviews_dir()).ok()
    } else {
        None
    };
    scan_skill_root_with_reviews(&root, source, reviews.as_ref())
}

fn scan_skill_root(root: &SecureDir, source: &str) -> Vec<SkillManifest> {
    let reviews = if source == "user" {
        SecureDir::open_existing(&activation_reviews_dir()).ok()
    } else {
        None
    };
    scan_skill_root_with_reviews(root, source, reviews.as_ref())
}

fn scan_skill_root_with_reviews(
    root: &SecureDir,
    source: &str,
    reviews: Option<&SecureDir>,
) -> Vec<SkillManifest> {
    let Ok(entries) = root.entry_names() else {
        return vec![];
    };
    let mut skills = Vec::new();
    for entry_name in entries {
        let path = root.path().join(&entry_name);
        let Ok(skill_dir) = root.open_child_dir(&entry_name) else {
            continue;
        };
        let folder_id = entry_name.to_str();
        let projected_id = folder_id
            .filter(|id| is_safe_skill_id(id))
            .map(str::to_string)
            .unwrap_or_else(|| corrupt_projection_id(&path));
        let manifest_result = skill_dir
            .read_string_required("manifest.json")
            .map_err(|error| format!("SKILL_MANIFEST_UNREADABLE: {error}"))
            .and_then(|raw| {
                serde_json::from_str::<ManifestFile>(&raw)
                    .map_err(|error| format!("SKILL_MANIFEST_INVALID: {error}"))
            });
        let mf = match manifest_result {
            Ok(mf) if is_safe_skill_id(&mf.id) && folder_id == Some(mf.id.as_str()) => mf,
            Ok(mf) => {
                skills.push(corrupt_skill_projection(
                    projected_id,
                    &path,
                    source,
                    format!(
                        "SKILL_MANIFEST_ID_MISMATCH: folder={:?}, manifest={:?}",
                        folder_id, mf.id
                    ),
                ));
                continue;
            }
            Err(error) => {
                skills.push(corrupt_skill_projection(projected_id, &path, source, error));
                continue;
            }
        };
        // Legacy user manifests predate the explicit review gate. Only the
        // package-external authority can approve the exact reviewed bytes; a
        // marker shipped inside an untrusted package has no effect.
        let expected_review = activation_review_fingerprint_in_dir(
            &skill_dir,
            &mf.id,
            &mf.name,
            &mf.description,
            &mf.version,
            &mf.author,
            &mf.tags,
        );
        let enabled = source == "user"
            && mf.enabled
            && expected_review.is_ok_and(|expected| {
                reviews
                    .and_then(|reviews| {
                        let filename = activation_review_filename(&mf.id).ok()?;
                        reviews.read_string_optional(&filename).ok().flatten()
                    })
                    .is_some_and(|actual| actual == expected)
            });
        skills.push(SkillManifest {
            id: mf.id,
            name: mf.name,
            description: mf.description,
            version: mf.version,
            author: mf.author,
            tags: mf.tags,
            enabled,
            path: path.to_string_lossy().to_string(),
            source: source.to_string(),
            lifecycle_status: "ready".to_string(),
        });
    }
    skills
}

fn corrupt_projection_id(path: &Path) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("corrupt-{hash:016x}")
}

fn corrupt_skill_projection(id: String, path: &Path, source: &str, error: String) -> SkillManifest {
    let folder = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| id.clone());
    SkillManifest {
        id,
        name: folder,
        description: error,
        version: "unknown".to_string(),
        author: "unknown".to_string(),
        tags: vec![],
        enabled: false,
        path: path.to_string_lossy().to_string(),
        source: source.to_string(),
        lifecycle_status: "corrupt".to_string(),
    }
}

#[cfg(test)]
fn activation_review_fingerprint(
    skill_dir: &Path,
    id: &str,
    name: &str,
    description: &str,
    version: &str,
    author: &str,
    tags: &[String],
) -> Result<String, String> {
    let skill_dir = SecureDir::open_existing(skill_dir)?;
    activation_review_snapshot_in_dir(&skill_dir, id, name, description, version, author, tags)
        .map(|snapshot| snapshot.fingerprint)
}

fn activation_review_fingerprint_in_dir(
    skill_dir: &SecureDir,
    id: &str,
    name: &str,
    description: &str,
    version: &str,
    author: &str,
    tags: &[String],
) -> Result<String, String> {
    activation_review_snapshot_in_dir(skill_dir, id, name, description, version, author, tags)
        .map(|snapshot| snapshot.fingerprint)
}

struct ActivationReviewSnapshot {
    fingerprint: String,
    system_prompt: Vec<u8>,
    slash_commands: Vec<u8>,
    tool_policy: Option<Vec<u8>>,
}

fn activation_review_snapshot_in_dir(
    skill_dir: &SecureDir,
    id: &str,
    name: &str,
    description: &str,
    version: &str,
    author: &str,
    tags: &[String],
) -> Result<ActivationReviewSnapshot, String> {
    let mut hasher = Sha256::new();
    let mut system_prompt = Vec::new();
    let mut slash_commands = Vec::new();
    let mut tool_policy = None;
    hasher.update(b"codefactory-skill-review-v1\0");
    for field in [id, name, description, version, author] {
        hasher.update((field.len() as u64).to_le_bytes());
        hasher.update(field.as_bytes());
    }
    hasher.update((tags.len() as u64).to_le_bytes());
    for tag in tags {
        hasher.update((tag.len() as u64).to_le_bytes());
        hasher.update(tag.as_bytes());
    }
    for filename in [
        "system_prompt.md",
        "slash_commands.json",
        "tool_policy.json",
    ] {
        let bytes = skill_dir
            .read_optional(filename)
            .map_err(|error| format!("SKILL_REVIEW_READ_FAILED: {filename}: {error}"))?
            .unwrap_or_default();
        if filename == "system_prompt.md" {
            system_prompt = bytes.clone();
        } else if filename == "slash_commands.json" {
            slash_commands = bytes.clone();
        } else if filename == "tool_policy.json" && !bytes.is_empty() {
            tool_policy = Some(bytes.clone());
        }
        hasher.update((filename.len() as u64).to_le_bytes());
        hasher.update(filename.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    }
    Ok(ActivationReviewSnapshot {
        fingerprint: format!("sha256:{:x}", hasher.finalize()),
        system_prompt,
        slash_commands,
        tool_policy,
    })
}

// ── Write manifest back to disk ───────────────────────────────────────────────

fn write_manifest_to_dir(skill_dir: &SecureDir, manifest: &SkillManifest) -> Result<(), String> {
    let mf = serde_json::json!({
        "id": manifest.id,
        "name": manifest.name,
        "description": manifest.description,
        "version": manifest.version,
        "author": manifest.author,
        "tags": manifest.tags,
        "enabled": manifest.enabled,
    });
    skill_dir.write_atomic(
        "manifest.json",
        serde_json::to_string_pretty(&mf)
            .unwrap_or_default()
            .as_bytes(),
    )
}

/// Copy a builtin skill directory to the user dir so we can mutate it.
fn copy_to_user_dir(skill_path: &str) -> Result<String, String> {
    let src_path = PathBuf::from(skill_path);
    let id = src_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "SKILL_ID_INVALID: builtin skill path has no portable id".to_string())?;
    if !is_safe_skill_id(id) {
        return Err(format!(
            "SKILL_ID_INVALID: builtin id is not portable: {id:?}"
        ));
    }
    let root = SecureDir::open_or_create(&user_skills_dir())?;
    if let Ok(existing) = root.open_child_dir(std::ffi::OsStr::new(id)) {
        return Ok(existing.path().to_string_lossy().to_string());
    }
    let dest = root.create_child_dir(id)?;
    let src = SecureDir::open_existing(&src_path)?;
    for filename in src.entry_names()? {
        let filename = filename
            .into_string()
            .map_err(|_| "SKILL_PATH_UNSAFE: non-UTF-8 package filename".to_string())?;
        if let Some(bytes) = src.read_optional(&filename)? {
            dest.write_atomic(&filename, &bytes)?;
        }
    }
    Ok(dest.path().to_string_lossy().to_string())
}

// ── Commands ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_skills(app: AppHandle) -> Result<Vec<SkillManifest>, String> {
    let mut map: std::collections::HashMap<String, SkillManifest> =
        std::collections::HashMap::new();

    // 1. Builtin skills
    if let Some(dir) = builtin_skills_dir(&app) {
        for s in scan_skill_dir(&dir, "builtin") {
            map.insert(s.id.clone(), s);
        }
    }

    // 2. User skills override builtins with same id
    let user_dir = user_skills_dir();
    for s in scan_skill_dir(&user_dir, "user") {
        map.insert(s.id.clone(), s);
    }

    let mut skills: Vec<SkillManifest> = map.into_values().collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}

#[tauri::command]
pub async fn get_skill(id: String, app: AppHandle) -> Result<SkillDetail, String> {
    let skills = list_skills(app).await?;
    let manifest = skills
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("Skill '{id}' not found"))?;

    if manifest.lifecycle_status != "ready" {
        return Ok(SkillDetail {
            manifest,
            system_prompt: String::new(),
            slash_commands: Vec::new(),
            has_tool_policy: false,
            tool_policy: None,
            review_fingerprint: None,
        });
    }

    let skill_dir = if manifest.source == "user" {
        let folder = Path::new(&manifest.path)
            .file_name()
            .ok_or_else(|| "SKILL_PATH_UNSAFE: installed package has no folder".to_string())?;
        SecureDir::open_existing(&user_skills_dir())?.open_child_dir(folder)?
    } else {
        SecureDir::open_existing(Path::new(&manifest.path))?
    };
    let snapshot = activation_review_snapshot_in_dir(
        &skill_dir,
        &manifest.id,
        &manifest.name,
        &manifest.description,
        &manifest.version,
        &manifest.author,
        &manifest.tags,
    )?;
    let system_prompt = String::from_utf8(snapshot.system_prompt)
        .map_err(|error| format!("SKILL_SYSTEM_PROMPT_INVALID: {error}"))?;
    let slash_commands = if snapshot.slash_commands.is_empty() {
        Vec::new()
    } else {
        serde_json::from_slice::<Vec<SlashCommand>>(&snapshot.slash_commands)
            .map_err(|error| format!("SKILL_SLASH_COMMANDS_INVALID: {error}"))?
    };
    let tool_policy = snapshot
        .tool_policy
        .map(|bytes| {
            String::from_utf8(bytes).map_err(|error| format!("SKILL_TOOL_POLICY_INVALID: {error}"))
        })
        .transpose()?;
    let has_tool_policy = tool_policy.is_some();

    Ok(SkillDetail {
        manifest,
        system_prompt,
        slash_commands,
        has_tool_policy,
        tool_policy,
        review_fingerprint: Some(snapshot.fingerprint),
    })
}

#[tauri::command]
pub async fn enable_skill(
    id: String,
    expected_review_fingerprint: String,
    app: AppHandle,
) -> Result<(), String> {
    validate_skill_id(&id)?;
    let skills = list_skills(app.clone()).await?;
    let mut manifest = skills
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("Skill '{id}' not found"))?;
    if manifest.lifecycle_status != "ready" {
        return Err(format!(
            "SKILL_CORRUPT_NOT_ACTIVATABLE: '{}' must be repaired or removed before enabling",
            manifest.id
        ));
    }

    // If builtin, copy to user dir first
    let skill_dir_handle = if manifest.source == "builtin" {
        let new_path = copy_to_user_dir(&manifest.path)?;
        manifest.path = new_path;
        manifest.source = "user".to_string();
        SecureDir::open_existing(&user_skills_dir())?
            .open_child_dir(std::ffi::OsStr::new(&manifest.id))?
    } else {
        SecureDir::open_existing(&user_skills_dir())?
            .open_child_dir(std::ffi::OsStr::new(&manifest.id))?
    };

    enable_user_skill_in(
        &skill_dir_handle,
        &activation_reviews_dir(),
        &mut manifest,
        &expected_review_fingerprint,
    )
}

fn enable_user_skill_in(
    skill_dir: &SecureDir,
    review_root: &Path,
    manifest: &mut SkillManifest,
    expected_review_fingerprint: &str,
) -> Result<(), String> {
    let current_review_fingerprint = activation_review_fingerprint_in_dir(
        skill_dir,
        &manifest.id,
        &manifest.name,
        &manifest.description,
        &manifest.version,
        &manifest.author,
        &manifest.tags,
    )?;
    if current_review_fingerprint != expected_review_fingerprint {
        return Err(
            "SKILL_REVIEW_CONTENT_CHANGED: Skill content changed after it was displayed; review the current version before enabling"
                .to_string(),
        );
    }
    let reviews = SecureDir::open_or_create(review_root)
        .map_err(|error| format!("SKILL_ACTIVATION_REVIEW_STORE_FAILED: {error}"))?;
    let review_filename = activation_review_filename(&manifest.id)?;
    reviews
        .write_atomic(&review_filename, current_review_fingerprint.as_bytes())
        .map_err(|error| format!("SKILL_ACTIVATION_REVIEW_STORE_FAILED: {error}"))?;
    manifest.enabled = true;
    write_manifest_to_dir(skill_dir, manifest)
}

#[tauri::command]
pub async fn disable_skill(id: String, app: AppHandle) -> Result<(), String> {
    validate_skill_id(&id)?;
    let skills = list_skills(app.clone()).await?;
    let mut manifest = skills
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("Skill '{id}' not found"))?;

    // If builtin, copy to user dir first
    let skill_dir_handle = if manifest.source == "builtin" {
        let new_path = copy_to_user_dir(&manifest.path)?;
        manifest.path = new_path;
        manifest.source = "user".to_string();
        SecureDir::open_existing(&user_skills_dir())?
            .open_child_dir(std::ffi::OsStr::new(&manifest.id))?
    } else {
        SecureDir::open_existing(&user_skills_dir())?
            .open_child_dir(std::ffi::OsStr::new(&manifest.id))?
    };

    disable_user_skill_in(&skill_dir_handle, &activation_reviews_dir(), &mut manifest)
}

fn disable_user_skill_in(
    skill_dir: &SecureDir,
    review_root: &Path,
    manifest: &mut SkillManifest,
) -> Result<(), String> {
    // Revoke approval before persisting the disabled bit. A crash or an
    // out-of-band manifest edit can then only leave the Skill ineligible.
    remove_activation_review(review_root, &manifest.id)?;
    manifest.enabled = false;
    write_manifest_to_dir(skill_dir, manifest)
}

#[tauri::command]
pub async fn install_skill_from_url(url: String, _app: AppHandle) -> Result<SkillManifest, String> {
    install_user_skill_from_url(&url).await
}

const MAX_REMOTE_MANIFEST_BYTES: usize = 1024 * 1024; // 1 MiB — a system prompt has no business being bigger.
const MAX_SKILL_ID_BYTES: usize = 64;
const REMOTE_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REMOTE_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// A skill id becomes a directory name under the user skills dir — reject
/// anything that isn't a plain path component (blocks path traversal via a
/// malicious/compromised remote manifest, e.g. `id: "../../.."`).
fn is_safe_skill_id(id: &str) -> bool {
    if id.is_empty()
        || id.len() > MAX_SKILL_ID_BYTES
        || !id.as_bytes()[0].is_ascii_lowercase() && !id.as_bytes()[0].is_ascii_digit()
        || id.ends_with(['.', ' '])
        || id.contains("..")
        || !id.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' || c == '.'
        })
    {
        return false;
    }

    let basename = id.split('.').next().unwrap_or_default();
    !matches!(
        basename,
        "con"
            | "prn"
            | "aux"
            | "nul"
            | "com1"
            | "com2"
            | "com3"
            | "com4"
            | "com5"
            | "com6"
            | "com7"
            | "com8"
            | "com9"
            | "lpt1"
            | "lpt2"
            | "lpt3"
            | "lpt4"
            | "lpt5"
            | "lpt6"
            | "lpt7"
            | "lpt8"
            | "lpt9"
    )
}

fn validate_skill_id(id: &str) -> Result<(), String> {
    if !is_safe_skill_id(id) {
        return Err(format!(
            "SKILL_ID_INVALID: skill id is not portable: {id:?}"
        ));
    }
    Ok(())
}

#[cfg(test)]
fn skill_path_under(root: &Path, id: &str) -> Result<PathBuf, String> {
    validate_skill_id(id)?;
    Ok(root.join(id))
}

#[cfg(test)]
fn prepare_skill_dir(root: &Path, id: &str) -> Result<PathBuf, String> {
    prepare_skill_dir_handle(root, id).map(|dir| dir.path().to_path_buf())
}

fn prepare_skill_dir_handle(root: &Path, id: &str) -> Result<SecureDir, String> {
    validate_skill_id(id)?;
    SecureDir::open_or_create(root)?.create_child_dir(id)
}

fn is_forbidden_remote_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _d] = ip.octets();
            a == 0
                || a == 10
                || a == 127
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 192 && b == 0 && c == 2)
                || (a == 192 && b == 88 && c == 99)
                || (a == 192 && b == 168)
                || (a == 198 && (b == 18 || b == 19))
                || (a == 198 && b == 51 && c == 100)
                || (a == 203 && b == 0 && c == 113)
                || a >= 224
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return is_forbidden_remote_ip(IpAddr::V4(mapped));
            }
            // Phase 0 cannot yet prove every IPv6 special-purpose allocation
            // is globally reachable. Fail closed until the resolver uses a
            // maintained global-address classifier.
            let _ = ip;
            true
        }
    }
}

fn validate_explicit_skill_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw).map_err(|e| format!("SKILL_SOURCE_URL_INVALID: {e}"))?;
    if url.scheme() != "https" {
        return Err("SKILL_SOURCE_HTTPS_REQUIRED: only public HTTPS sources are supported".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("SKILL_SOURCE_URL_INVALID: credentials are not allowed in source URLs".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(
            "SKILL_SOURCE_URL_SENSITIVE_COMPONENT_BLOCKED: query and fragment are not supported"
                .into(),
        );
    }
    let host = url
        .host_str()
        .ok_or_else(|| "SKILL_SOURCE_URL_INVALID: source URL has no host".to_string())?;
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") || lower.ends_with(".local") {
        return Err("SKILL_SOURCE_PRIVATE_NETWORK: local hosts are blocked".into());
    }
    let literal_ip = match url.host() {
        Some(url::Host::Ipv4(ip)) => Some(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => Some(IpAddr::V6(ip)),
        _ => None,
    };
    if literal_ip.is_some_and(is_forbidden_remote_ip) {
        return Err("SKILL_SOURCE_PRIVATE_NETWORK: private network targets are blocked".into());
    }
    Ok(url)
}

async fn resolve_public_source(url: &reqwest::Url) -> Result<(String, SocketAddr), String> {
    let host = url
        .host_str()
        .ok_or_else(|| "SKILL_SOURCE_URL_INVALID: source URL has no host".to_string())?
        .to_string();
    let port = url.port_or_known_default().unwrap_or(443);
    let lookup_host = host.clone();
    let resolved = tokio::time::timeout(
        REMOTE_CONNECT_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            (lookup_host.as_str(), port)
                .to_socket_addrs()
                .map(|iter| iter.collect::<Vec<_>>())
        }),
    )
    .await
    .map_err(|_| "SKILL_SOURCE_TIMEOUT: DNS resolution timed out".to_string())?
    .map_err(|e| format!("SKILL_SOURCE_DNS_FAILED: {e}"))?
    .map_err(|e| format!("SKILL_SOURCE_DNS_FAILED: {e}"))?;
    let address = resolved
        .into_iter()
        .find(|address| !is_forbidden_remote_ip(address.ip()))
        .ok_or_else(|| {
            "SKILL_SOURCE_PRIVATE_NETWORK: source resolved only to blocked addresses".to_string()
        })?;
    Ok((host, address))
}

async fn fetch_bounded_public_https(raw_url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    let url = validate_explicit_skill_url(raw_url)?;
    let (host, address) = resolve_public_source(&url).await?;
    let client = reqwest::Client::builder()
        // A system proxy would bypass the DNS-pinned socket address above.
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(REMOTE_CONNECT_TIMEOUT)
        .timeout(REMOTE_REQUEST_TIMEOUT)
        .resolve(&host, address)
        .build()
        .map_err(|e| format!("SKILL_SOURCE_CLIENT_FAILED: {e}"))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("SKILL_SOURCE_FETCH_FAILED: {e}"))?;
    if response.status().is_redirection() {
        return Err("SKILL_SOURCE_REDIRECT_BLOCKED: redirects are not followed".into());
    }
    if !response.status().is_success() {
        return Err(format!("SKILL_SOURCE_HTTP_STATUS: {}", response.status()));
    }
    if response
        .content_length()
        .is_some_and(|size| size > max_bytes as u64)
    {
        return Err(format!(
            "SKILL_SOURCE_TOO_LARGE: response exceeds {max_bytes} bytes"
        ));
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("SKILL_SOURCE_READ_FAILED: {e}"))?;
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(format!(
                "SKILL_SOURCE_TOO_LARGE: response exceeds {max_bytes} bytes"
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Fetch a JSON skill manifest from `url` and write it to the user skills dir.
/// External installs always land disabled for explicit review. App-independent.
pub async fn install_user_skill_from_url(url: &str) -> Result<SkillManifest, String> {
    let bytes = fetch_bounded_public_https(url, MAX_REMOTE_MANIFEST_BYTES).await?;
    let raw = String::from_utf8(bytes).map_err(|e| format!("SKILL_SOURCE_UTF8_INVALID: {e}"))?;

    let mf: ManifestFile =
        serde_json::from_str(&raw).map_err(|e| format!("SKILL_MANIFEST_INVALID: {e}"))?;
    if !is_safe_skill_id(&mf.id) {
        return Err(format!(
            "SKILL_ID_INVALID: manifest id is not portable: {:?}",
            mf.id
        ));
    }

    let skill_dir = prepare_skill_dir_handle(&user_skills_dir(), &mf.id)?;
    skill_dir.write_atomic(
        "system_prompt.md",
        mf.system_prompt.clone().unwrap_or_default().as_bytes(),
    )?;

    let manifest = SkillManifest {
        id: mf.id,
        name: mf.name,
        description: mf.description,
        version: mf.version,
        author: mf.author,
        tags: mf.tags,
        enabled: false,
        path: skill_dir.path().to_string_lossy().to_string(),
        source: "user".to_string(),
        lifecycle_status: "ready".to_string(),
    };
    write_manifest_to_dir(&skill_dir, &manifest)?;
    Ok(manifest)
}

// ── Local-directory import (SKILL.md / manifest.json) ─────────────────────────

/// Frontmatter + body parsed out of a `SKILL.md` (the standard Claude skill
/// format used by superpowers / openspec / etc.).
struct ParsedSkillMd {
    id: String,
    name: String,
    description: String,
    tags: Vec<String>,
    body: String,
}

/// Best-effort parse of a `SKILL.md`: a leading `--- … ---` frontmatter (we read
/// name / description / id / tags) followed by the markdown body, which becomes
/// the skill's system prompt. No YAML dependency — these manifests use simple
/// `key: value` lines.
fn parse_skill_md(content: &str) -> ParsedSkillMd {
    let content = content.trim_start_matches('\u{feff}'); // strip BOM
    let (frontmatter, body) = {
        let t = content.trim_start();
        match t
            .strip_prefix("---")
            .and_then(|after| after.find("\n---").map(|c| (after, c)))
        {
            Some((after, close)) => {
                let fm = after[..close].to_string();
                let rest = after[close + 4..].trim_start_matches('-');
                (fm, rest.trim_start_matches(['\r', '\n']).to_string())
            }
            None => (String::new(), content.to_string()),
        }
    };

    let mut id = String::new();
    let mut name = String::new();
    let mut description = String::new();
    let mut tags: Vec<String> = Vec::new();
    for line in frontmatter.lines() {
        if let Some((k, v)) = line.trim().split_once(':') {
            let val = v.trim().trim_matches(['"', '\'']).to_string();
            match k.trim().to_ascii_lowercase().as_str() {
                "name" => name = val,
                "description" => description = val,
                "id" | "slug" => id = val,
                "tags" => {
                    tags = val
                        .trim_matches(['[', ']'])
                        .split(',')
                        .map(|s| s.trim().trim_matches(['"', '\'']).to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                _ => {}
            }
        }
    }
    ParsedSkillMd {
        id,
        name,
        description,
        tags,
        body,
    }
}

fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in s.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let s = out.trim_matches('-').to_string();
    if s.is_empty() {
        "imported-skill".to_string()
    } else {
        s
    }
}

fn is_skill_dir(d: &SecureDir) -> bool {
    d.has_regular_file("SKILL.md") || d.has_regular_file("manifest.json")
}

const MAX_SKILL_PAYLOAD_FILES: usize = 2_048;
const MAX_SKILL_PAYLOAD_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SKILL_PAYLOAD_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SKILL_PAYLOAD_DEPTH: usize = 16;
const MAX_SKILL_PAYLOAD_ENTRIES: usize = 4_096;

/// Copy the static bundle a Skill was authored with. Installation never
/// executes payload files and never follows links; `.git` is source-control
/// state, not part of the runnable Skill.
#[derive(Default)]
struct SkillPayloadBudget {
    files: usize,
    bytes: u64,
    entries: usize,
}

fn visit_skill_payload(
    src: &SecureDir,
    dest: Option<&SecureDir>,
    depth: usize,
    budget: &mut SkillPayloadBudget,
) -> Result<(), String> {
    for name in src.entry_names()? {
        if name == ".git" || (depth == 0 && name == "manifest.json") {
            continue;
        }
        budget.entries = budget.entries.saturating_add(1);
        if budget.entries > MAX_SKILL_PAYLOAD_ENTRIES || depth >= MAX_SKILL_PAYLOAD_DEPTH {
            return Err("Skill payload 超过安全目录深度或条目数上限".into());
        }
        if let Ok(child) = src.open_child_dir(&name) {
            let child_name = name
                .to_str()
                .ok_or_else(|| "SKILL_PATH_UNSAFE: non-UTF-8 payload directory".to_string())?;
            let destination = dest
                .map(|parent| parent.create_child_dir(child_name))
                .transpose()?;
            visit_skill_payload(&child, destination.as_ref(), depth + 1, budget)?;
            continue;
        }
        let filename = name
            .into_string()
            .map_err(|_| "SKILL_PATH_UNSAFE: non-UTF-8 payload filename".to_string())?;
        let bytes = src
            .read_optional(&filename)?
            .ok_or_else(|| format!("SKILL_PATH_UNSAFE: unsupported payload entry {filename:?}"))?;
        budget.files = budget.files.saturating_add(1);
        budget.bytes = budget.bytes.saturating_add(bytes.len() as u64);
        if budget.files > MAX_SKILL_PAYLOAD_FILES
            || bytes.len() as u64 > MAX_SKILL_PAYLOAD_FILE_BYTES
            || budget.bytes > MAX_SKILL_PAYLOAD_BYTES
        {
            return Err("Skill payload 超过安全复制上限".into());
        }
        if let Some(destination) = dest {
            destination.write_atomic(&filename, &bytes)?;
        }
    }
    Ok(())
}

fn validate_skill_payload(src: &SecureDir) -> Result<(), String> {
    visit_skill_payload(src, None, 0, &mut SkillPayloadBudget::default())
}

fn copy_skill_payload(src: &SecureDir, dest: &SecureDir) -> Result<(), String> {
    visit_skill_payload(src, Some(dest), 0, &mut SkillPayloadBudget::default())
}

/// Walk `dir` (bounded depth) collecting every skill directory — one holding a
/// SKILL.md or manifest.json. A skill dir is a leaf (we don't descend into it),
/// so a single skill, a `skills/<name>/` collection, or a whole repo all work.
/// Hidden dirs (.git, …) are skipped.
fn collect_skill_dirs(dir: SecureDir, depth: u8, out: &mut Vec<SecureDir>) {
    if is_skill_dir(&dir) {
        out.push(dir);
        return;
    }
    if depth == 0 {
        return;
    }
    if let Ok(entries) = dir.entry_names() {
        for name in entries {
            let hidden = name.to_string_lossy().starts_with('.');
            if !hidden {
                if let Ok(child) = dir.open_child_dir(&name) {
                    collect_skill_dirs(child, depth - 1, out);
                }
            }
        }
    }
}

/// Import one skill directory into the user skills folder. Returns its manifest.
fn import_one_skill_dir_into(
    src: &SecureDir,
    install_root: &Path,
) -> Result<SkillManifest, String> {
    let (id_raw, name, description, version, author, tags, body): (
        String,
        String,
        String,
        String,
        String,
        Vec<String>,
        Option<String>,
    ) = if let Some(raw) = src.read_string_optional("manifest.json")? {
        let mf: ManifestFile =
            serde_json::from_str(&raw).map_err(|e| format!("manifest.json 解析失败: {e}"))?;
        (
            mf.id,
            mf.name,
            mf.description,
            mf.version,
            mf.author,
            mf.tags,
            mf.system_prompt,
        )
    } else {
        let raw = src.read_string_required("SKILL.md")?;
        let p = parse_skill_md(&raw);
        (
            p.id,
            p.name,
            p.description,
            "1.0.0".into(),
            "imported".into(),
            p.tags,
            Some(p.body),
        )
    };

    let dir_name = src
        .path()
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let id = if !id_raw.trim().is_empty() {
        // An explicit package id is a security boundary, not display text.
        // Reject traversal and non-canonical values instead of silently
        // transforming an attack string into an installable id.
        if id_raw != id_raw.trim() {
            return Err(format!(
                "SKILL_ID_INVALID: explicit package id is not canonical: {id_raw:?}"
            ));
        }
        validate_skill_id(&id_raw)?;
        id_raw
    } else if !name.trim().is_empty() {
        slugify(&name)
    } else {
        slugify(&dir_name)
    };

    validate_skill_id(&id)?;
    // Validate the complete source before creating or touching the target. The
    // same no-follow traversal is repeated during the copy so a source race
    // cannot bypass the package limits or introduce a link between passes.
    validate_skill_payload(src)?;
    let dest = prepare_skill_dir_handle(install_root, &id)?;

    let manifest = SkillManifest {
        id: id.clone(),
        name: if name.trim().is_empty() {
            id.clone()
        } else {
            name
        },
        description,
        version,
        author,
        tags,
        enabled: false,
        path: dest.path().to_string_lossy().to_string(),
        source: "user".to_string(),
        lifecycle_status: "ready".to_string(),
    };
    if let Err(error) = copy_skill_payload(src, &dest) {
        let _ = dest.remove_open_dir_all();
        return Err(error);
    }
    // system_prompt.md: parsed SKILL.md body wins; otherwise the copied
    // system_prompt.md is retained.
    if let Some(body) = body {
        dest.write_atomic("system_prompt.md", body.as_bytes())?;
    }
    // The manifest is the visibility/eligibility commit point. Write it last
    // so a failed copy is projected as `corrupt`, never as an installed Skill.
    write_manifest_to_dir(&dest, &manifest)?;
    Ok(manifest)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SkillImportFailure {
    pub path: String,
    pub error: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SkillImportResult {
    pub succeeded: Vec<SkillManifest>,
    pub failed: Vec<SkillImportFailure>,
}

fn import_skill_directories_into(
    source: &Path,
    install_root: &Path,
) -> Result<SkillImportResult, String> {
    let source_dir =
        SecureDir::open_existing(source).map_err(|e| format!("SKILL_SOURCE_NOT_DIRECTORY: {e}"))?;
    import_skill_directories_from_handle(source_dir, install_root)
}

fn import_skill_directories_from_handle(
    source_dir: SecureDir,
    install_root: &Path,
) -> Result<SkillImportResult, String> {
    let mut dirs = Vec::new();
    collect_skill_dirs(source_dir, 3, &mut dirs);
    if dirs.is_empty() {
        return Err(
            "SKILL_SOURCE_EMPTY: 目录里没找到 skill（需要含 SKILL.md 或 manifest.json 的文件夹）"
                .to_string(),
        );
    }
    let mut succeeded = Vec::new();
    let mut failed = Vec::new();
    for dir in dirs {
        let source_path = dir.path().to_string_lossy().to_string();
        match import_one_skill_dir_into(&dir, install_root) {
            Ok(manifest) => succeeded.push(manifest),
            Err(error) => failed.push(SkillImportFailure {
                path: source_path,
                error,
            }),
        }
    }
    Ok(SkillImportResult { succeeded, failed })
}

/// Import skill(s) from a local directory — a single skill folder, a
/// `skills/<name>/` collection, or a whole repo. Parses `SKILL.md` frontmatter
/// (superpowers / openspec format) when there's no `manifest.json`.
#[tauri::command]
pub async fn install_skill_from_directory(
    source_handle: String,
) -> Result<SkillImportResult, String> {
    let source = consume_skill_source(&source_handle)?;
    import_skill_directories_from_handle(source, &user_skills_dir())
}

// ── OpenClaw one-click import (scan known roots) ──────────────────────────────

/// Preview row for a skill discovered in an OpenClaw/Claude skills root.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OpenClawSkillPreview {
    pub name: String,
    pub description: String,
    pub path: String,
    /// Opaque, short-lived, single-use capability for importing this exact
    /// already-open directory. The renderer never submits a local path.
    pub source_handle: String,
    /// A skill with the same slug already exists locally — the UI greys it
    /// out instead of double-importing.
    pub already_installed: bool,
}

/// The known on-disk locations OpenClaw / Claude Code keep skills in.
/// Missing directories are fine — the scanner skips them.
pub fn openclaw_skill_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = dirs_home() {
        roots.push(home.join(".openclaw").join("workspace").join("skills"));
        roots.push(home.join(".claude").join("skills"));
    }
    roots
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Scan `roots` for skill directories (SKILL.md / manifest.json) and build
/// previews. `installed_ids` are the local skill slugs used to mark
/// duplicates. Each preview receives an opaque capability for the exact open
/// directory, so importing never re-resolves a renderer-provided path.
pub fn scan_skill_roots(roots: &[PathBuf], installed_ids: &[String]) -> Vec<OpenClawSkillPreview> {
    let mut previews = Vec::new();
    for root in roots {
        let Ok(root_dir) = SecureDir::open_existing(root) else {
            continue;
        };
        let mut dirs = Vec::new();
        collect_skill_dirs(root_dir, 2, &mut dirs);
        for dir in dirs {
            let (name, description) = if let Ok(Some(raw)) = dir.read_string_optional("SKILL.md") {
                let parsed = parse_skill_md(&raw);
                let fallback = dir
                    .path()
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                (
                    if parsed.name.trim().is_empty() {
                        fallback
                    } else {
                        parsed.name
                    },
                    parsed.description,
                )
            } else {
                match dir
                    .read_string_optional("manifest.json")
                    .ok()
                    .flatten()
                    .and_then(|raw| serde_json::from_str::<ManifestFile>(&raw).ok())
                {
                    Some(mf) => (mf.name, mf.description),
                    None => continue,
                }
            };
            let slug = slugify(&name);
            let path = dir.path().to_string_lossy().to_string();
            let Ok(selection) = register_skill_source(dir) else {
                continue;
            };
            previews.push(OpenClawSkillPreview {
                already_installed: installed_ids.contains(&slug),
                name,
                description,
                path,
                source_handle: selection.source_handle,
            });
        }
    }
    previews
}

/// Scan the known OpenClaw/Claude roots and preview importable skills.
#[tauri::command]
pub async fn scan_openclaw_skills() -> Result<Vec<OpenClawSkillPreview>, String> {
    let installed: Vec<String> = list_user_and_builtin_skill_ids();
    Ok(scan_skill_roots(&openclaw_skill_roots(), &installed))
}

fn list_user_and_builtin_skill_ids() -> Vec<String> {
    let mut ids = Vec::new();
    let dir = user_skills_dir();
    if let Ok(root) = SecureDir::open_existing(&dir) {
        for entry in root.entry_names().unwrap_or_default() {
            if root.open_child_dir(&entry).is_ok() {
                ids.push(entry.to_string_lossy().to_string());
            }
        }
    }
    ids
}

// ── Reusable, app-independent skill ops (shared by agent tools) ───────────────

/// Create a new USER skill on disk. Only touches the user skills dir (no
/// AppHandle needed), so the agent's skill tools can call it. Errors if the id
/// already exists — use [`update_user_skill`] to change one.
pub fn create_user_skill(
    name: &str,
    description: &str,
    instructions: &str,
) -> Result<SkillManifest, String> {
    let id = slugify(name);
    let dest = prepare_skill_dir_handle(&user_skills_dir(), &id).map_err(|error| {
        if error.starts_with("SKILL_ID_ALREADY_INSTALLED") {
            format!("技能 '{id}' 已存在，请改用更新（skill_update）")
        } else {
            error
        }
    })?;
    let manifest = SkillManifest {
        id: id.clone(),
        name: name.to_string(),
        description: description.to_string(),
        version: "1.0.0".to_string(),
        author: "you".to_string(),
        tags: vec![],
        enabled: false,
        path: dest.path().to_string_lossy().to_string(),
        source: "user".to_string(),
        lifecycle_status: "ready".to_string(),
    };
    dest.write_atomic("system_prompt.md", instructions.as_bytes())?;
    write_manifest_to_dir(&dest, &manifest)?;
    Ok(manifest)
}

/// Update an existing USER skill's fields / instructions. Each `Some` is applied;
/// `None` leaves that field as-is. Any edit returns the skill to disabled so the
/// changed content cannot affect a later turn before explicit re-review.
pub fn update_user_skill(
    id: &str,
    name: Option<&str>,
    description: Option<&str>,
    instructions: Option<&str>,
) -> Result<SkillManifest, String> {
    let updated = update_user_skill_in(&user_skills_dir(), id, name, description, instructions)?;
    remove_activation_review(&activation_reviews_dir(), id)?;
    Ok(updated)
}

fn update_user_skill_in(
    root: &Path,
    id: &str,
    name: Option<&str>,
    description: Option<&str>,
    instructions: Option<&str>,
) -> Result<SkillManifest, String> {
    if !is_safe_skill_id(id) {
        return Err(format!(
            "SKILL_ID_INVALID: skill id is not portable: {id:?}"
        ));
    }
    let root_dir = SecureDir::open_existing(root)?;
    let dir = root_dir
        .open_child_dir(std::ffi::OsStr::new(id))
        .map_err(|_| format!("用户技能 '{id}' 不存在或路径不安全"))?;
    let raw = dir.read_string_required("manifest.json")?;
    let mf: ManifestFile =
        serde_json::from_str(&raw).map_err(|e| format!("manifest.json 解析失败: {e}"))?;
    let manifest = SkillManifest {
        id: id.to_string(),
        name: name.map(String::from).unwrap_or(mf.name),
        description: description.map(String::from).unwrap_or(mf.description),
        version: mf.version,
        author: mf.author,
        tags: mf.tags,
        // Any content or metadata revision must be explicitly inspected again.
        // Persist the disabled manifest before writing instructions so even an
        // interrupted content write cannot leave an old enabled bit pointing at
        // a partially updated prompt.
        enabled: false,
        path: dir.path().to_string_lossy().to_string(),
        source: "user".to_string(),
        lifecycle_status: "ready".to_string(),
    };
    write_manifest_to_dir(&dir, &manifest)?;
    if let Some(instr) = instructions {
        dir.write_atomic("system_prompt.md", instr.as_bytes())?;
    }
    Ok(manifest)
}

/// List the USER skills (those under the user skills dir). App-independent.
pub fn list_user_skills() -> Vec<SkillManifest> {
    scan_skill_dir(&user_skills_dir(), "user")
}

/// Delete a USER skill by id. App-independent.
pub fn delete_user_skill(id: &str) -> Result<(), String> {
    remove_activation_review(&activation_reviews_dir(), id)?;
    delete_user_skill_in(&user_skills_dir(), id)
}

fn remove_activation_review(review_root: &Path, id: &str) -> Result<(), String> {
    let filename = activation_review_filename(id)?;
    let reviews = match SecureDir::open_existing(review_root) {
        Ok(reviews) => reviews,
        Err(_) if !review_root.exists() => return Ok(()),
        Err(error) => return Err(format!("SKILL_ACTIVATION_REVIEW_STORE_FAILED: {error}")),
    };
    reviews
        .remove_file(&filename)
        .map_err(|error| format!("SKILL_ACTIVATION_REVIEW_STORE_FAILED: {error}"))
}

fn delete_user_skill_in(root: &Path, id: &str) -> Result<(), String> {
    if !is_safe_skill_id(id) {
        return Err(format!(
            "SKILL_ID_INVALID: skill id is not portable: {id:?}"
        ));
    }
    let root_dir = SecureDir::open_existing(root)?;
    let dir = root_dir
        .open_child_dir(std::ffi::OsStr::new(id))
        .map_err(|_| format!("用户技能 '{id}' 不存在或路径不安全"))?;
    dir.remove_open_dir_all()
}

/// Create a skill from the UI form. It enters the same disabled review flow as imports.
#[tauri::command]
pub async fn create_skill(
    name: String,
    description: String,
    instructions: String,
) -> Result<SkillManifest, String> {
    create_user_skill(&name, &description, &instructions)
}

/// Update a USER skill from the UI form. Only the provided fields change.
#[tauri::command]
pub async fn update_skill(
    id: String,
    name: Option<String>,
    description: Option<String>,
    instructions: Option<String>,
) -> Result<SkillManifest, String> {
    update_user_skill(
        &id,
        name.as_deref(),
        description.as_deref(),
        instructions.as_deref(),
    )
}

#[tauri::command]
pub async fn delete_skill(id: String, app: AppHandle) -> Result<(), String> {
    let skills = list_skills(app).await?;
    let manifest = skills
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("Skill '{id}' not found"))?;

    if manifest.source != "user" {
        return Err("Cannot delete built-in skills. Disable them instead.".to_string());
    }

    // Deleting a still-disabled proposed skill is the user's rejection signal
    // (see propose_skills_from_patterns) — record it so the same cluster
    // never gets re-proposed.
    if !manifest.enabled && manifest.tags.iter().any(|t| t == "proposed") {
        record_rejected_proposal_key(&manifest.name)?;
    }

    let folder = Path::new(&manifest.path)
        .file_name()
        .ok_or_else(|| "SKILL_PATH_UNSAFE: installed package has no folder".to_string())?;
    let root = SecureDir::open_existing(&user_skills_dir())?;
    remove_activation_review(&activation_reviews_dir(), &manifest.id)?;
    root.open_child_dir(folder)?.remove_open_dir_all()
}

/// The trimmed `system_prompt.md` body of every enabled skill, in list order.
/// The agent loop wraps each into a budgeted context block (see
/// `agent::context_budget`) rather than concatenating them unbounded.
/// Assemble enabled-skill system prompts from a single skills directory —
/// no `AppHandle` needed. The headless agent loop uses this (user skills
/// only); the UI path merges builtin + user via [`enabled_skill_prompts`].
pub fn prompts_from_skill_dir(dir: &Path) -> Vec<String> {
    prompts_from_skill_dir_with_reviews(dir, Some(&activation_reviews_dir()))
}

fn prompts_from_skill_dir_with_reviews(dir: &Path, review_dir: Option<&Path>) -> Vec<String> {
    let Ok(root) = SecureDir::open_existing(dir) else {
        return Vec::new();
    };
    let reviews = review_dir.and_then(|path| SecureDir::open_existing(path).ok());
    scan_skill_root_with_reviews(&root, "user", reviews.as_ref())
        .iter()
        .filter(|s| s.enabled)
        .filter_map(|manifest| {
            reviewed_prompt_for_manifest_in_root(manifest, &root, reviews.as_ref())
        })
        .filter(|p| !p.is_empty())
        .collect()
}

fn reviewed_prompt_for_manifest(manifest: &SkillManifest) -> Option<String> {
    if !manifest.enabled || manifest.lifecycle_status != "ready" {
        return None;
    }
    let skill_dir = if manifest.source == "user" {
        SecureDir::open_existing(&user_skills_dir())
            .ok()?
            .open_child_dir(std::ffi::OsStr::new(&manifest.id))
            .ok()?
    } else {
        SecureDir::open_existing(Path::new(&manifest.path)).ok()?
    };
    let reviews = if manifest.source == "user" {
        SecureDir::open_existing(&activation_reviews_dir()).ok()
    } else {
        None
    };
    reviewed_prompt_from_dir(manifest, &skill_dir, reviews.as_ref())
}

fn reviewed_prompt_for_manifest_in_root(
    manifest: &SkillManifest,
    root: &SecureDir,
    reviews: Option<&SecureDir>,
) -> Option<String> {
    if !manifest.enabled || manifest.lifecycle_status != "ready" {
        return None;
    }
    let skill_dir = root
        .open_child_dir(std::ffi::OsStr::new(&manifest.id))
        .ok()?;
    reviewed_prompt_from_dir(manifest, &skill_dir, reviews)
}

fn reviewed_prompt_from_dir(
    manifest: &SkillManifest,
    skill_dir: &SecureDir,
    reviews: Option<&SecureDir>,
) -> Option<String> {
    let snapshot = activation_review_snapshot_in_dir(
        skill_dir,
        &manifest.id,
        &manifest.name,
        &manifest.description,
        &manifest.version,
        &manifest.author,
        &manifest.tags,
    )
    .ok()?;
    if manifest.source == "user" {
        let filename = activation_review_filename(&manifest.id).ok()?;
        let actual = reviews?.read_string_optional(&filename).ok().flatten()?;
        if actual != snapshot.fingerprint {
            return None;
        }
    }
    let body = String::from_utf8(snapshot.system_prompt).ok()?;
    let body = body.trim();
    if body.is_empty() {
        return None;
    }
    let root = skill_dir.path().canonicalize().ok()?;
    Some(format!(
        "<enabled_skill>\nskill_id: {}\nskill_root: {}\nResolve scripts/, references/, and assets/ relative to skill_root; do not resolve them from the project working directory.\n{}\n</enabled_skill>",
        manifest.id,
        root.display(),
        body,
    ))
}

/// Enabled USER-skill prompts, headless (no builtin skills, no `AppHandle`).
pub async fn enabled_user_skill_prompts() -> Vec<String> {
    prompts_from_skill_dir(&user_skills_dir())
}

pub async fn enabled_skill_prompts(app: &AppHandle) -> Vec<String> {
    let skills = match list_skills(app.clone()).await {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    skills
        .iter()
        .filter(|s| s.enabled)
        .filter_map(reviewed_prompt_for_manifest)
        .filter(|p| !p.is_empty())
        .collect()
}

// ── Marketplace ───────────────────────────────────────────────────────────────

const BUILTIN_REGISTRY: &str = r#"[
  {
    "id": "python-expert",
    "name": "Python 专家",
    "description": "Python 开发专家，熟悉类型注解、异步编程、测试",
    "version": "1.0.0",
    "author": "CodeFactory",
    "tags": ["python", "backend"],
    "download_url": null,
    "system_prompt": "You are an expert Python developer. Use type hints everywhere. Prefer pathlib over os.path. Write async code with asyncio when appropriate. Always suggest tests.",
    "slash_commands": [
      { "name": "dataclass", "description": "生成 Python dataclass", "template": "Create a Python dataclass for: {input}" },
      { "name": "async-fn", "description": "生成 async 函数", "template": "Create an async Python function for: {input}" }
    ]
  },
  {
    "id": "sql-assistant",
    "name": "SQL 助手",
    "description": "SQL 查询优化、数据库设计、迁移",
    "version": "1.0.0",
    "author": "CodeFactory",
    "tags": ["sql", "database"],
    "download_url": null,
    "system_prompt": "You are a SQL expert. Write efficient queries. Always consider indexes. Prefer CTEs over nested subqueries for readability. Warn about N+1 query patterns.",
    "slash_commands": [
      { "name": "query", "description": "优化 SQL 查询", "template": "Optimize this SQL query: {input}" },
      { "name": "migration", "description": "生成数据库迁移", "template": "Create a database migration for: {input}" }
    ]
  },
  {
    "id": "devops-helper",
    "name": "DevOps 助手",
    "description": "Docker、CI/CD、基础设施即代码",
    "version": "1.0.0",
    "author": "CodeFactory",
    "tags": ["devops", "docker", "ci"],
    "download_url": null,
    "system_prompt": "You are a DevOps engineer. Write minimal, secure Dockerfiles. Design idempotent CI pipelines. Prefer declarative infrastructure. Always consider secret management.",
    "slash_commands": [
      { "name": "dockerfile", "description": "生成 Dockerfile", "template": "Create a Dockerfile for: {input}" },
      { "name": "github-action", "description": "生成 GitHub Actions workflow", "template": "Create a GitHub Actions workflow for: {input}" }
    ]
  },
  {
    "id": "code-reviewer",
    "name": "代码审查",
    "description": "审查改动里的正确性 bug、安全隐患、性能与可读性问题",
    "version": "1.0.0",
    "author": "CodeFactory",
    "tags": ["review", "quality", "popular"],
    "download_url": null,
    "system_prompt": "You are a meticulous code reviewer. For each change, look for correctness bugs (off-by-one, null/None, race conditions, error handling), security issues, performance regressions, and readability. Cite exact file and line. Distinguish must-fix from nice-to-have. Be specific and concise; do not nitpick style a formatter would catch.",
    "slash_commands": [
      { "name": "review", "description": "审查这段改动/代码", "template": "Review the following code for bugs, security, and clarity. List findings with severity:\n{input}" },
      { "name": "review-diff", "description": "审查当前 git diff", "template": "Run the project's diff command, then review the changes for correctness and security. Focus on: {input}" }
    ]
  },
  {
    "id": "security-auditor",
    "name": "安全审计",
    "description": "按 OWASP 排查注入、鉴权、密钥泄露、不安全反序列化等",
    "version": "1.0.0",
    "author": "CodeFactory",
    "tags": ["security", "owasp", "popular"],
    "download_url": null,
    "system_prompt": "You are a security auditor. Hunt for injection (SQL/command/path), broken auth and access control, hardcoded secrets and keys, unsafe deserialization, SSRF, and missing input validation. Map findings to OWASP categories, rate severity, and give a concrete remediation. Never invent vulnerabilities; if unsure, say what to check.",
    "slash_commands": [
      { "name": "audit", "description": "安全审计这段代码", "template": "Security-audit this code. Report each issue with OWASP category, severity, and a fix:\n{input}" },
      { "name": "secrets", "description": "扫描可能泄露的密钥", "template": "Scan for hardcoded secrets, tokens, or credentials in: {input}" }
    ]
  },
  {
    "id": "test-engineer",
    "name": "测试工程师",
    "description": "写聚焦的单元测试,覆盖边界、错误路径与并发",
    "version": "1.0.0",
    "author": "CodeFactory",
    "tags": ["testing", "tdd", "popular"],
    "download_url": null,
    "system_prompt": "You are a testing expert. Write focused, deterministic tests that cover the happy path, boundary conditions, error paths, and concurrency where relevant. Prefer the project's existing test framework and conventions. One behavior per test, clear names, no flaky timing. Suggest the smallest set of cases that meaningfully raises confidence.",
    "slash_commands": [
      { "name": "test", "description": "为这段代码写测试", "template": "Write thorough unit tests (happy path + edge cases + error paths) for:\n{input}" },
      { "name": "edge-cases", "description": "列出该功能的边界用例", "template": "List the edge cases and failure modes worth testing for: {input}" }
    ]
  },
  {
    "id": "refactor-expert",
    "name": "重构专家",
    "description": "在不改变行为的前提下简化、去重、提升可读性",
    "version": "1.0.0",
    "author": "CodeFactory",
    "tags": ["refactor", "clean-code"],
    "download_url": null,
    "system_prompt": "You are a refactoring expert. Improve clarity and remove duplication without changing behavior. Prefer small, safe steps; keep the public interface stable unless asked. Reach for early returns, well-named helpers, and pure functions. Call out any change that is behavior-affecting so it can be verified.",
    "slash_commands": [
      { "name": "refactor", "description": "重构这段代码", "template": "Refactor this code for clarity and reuse, preserving behavior. Explain each change:\n{input}" },
      { "name": "simplify", "description": "简化复杂逻辑", "template": "Simplify this logic without changing what it does: {input}" }
    ]
  },
  {
    "id": "react-expert",
    "name": "React 专家",
    "description": "Hooks、渲染性能、可访问性与状态管理",
    "version": "1.0.0",
    "author": "CodeFactory",
    "tags": ["react", "frontend"],
    "download_url": null,
    "system_prompt": "You are a senior React engineer. Write idiomatic function components with correct hook dependencies, stable keys, and minimal re-renders (memo/useCallback only where it pays off). Mind accessibility (roles, labels, keyboard) and avoid unnecessary state. Prefer composition over prop drilling; lift state only as far as needed.",
    "slash_commands": [
      { "name": "component", "description": "生成一个 React 组件", "template": "Create an accessible, idiomatic React + TypeScript component for: {input}" },
      { "name": "perf", "description": "诊断渲染性能", "template": "Diagnose and fix re-render / performance issues in this React code: {input}" }
    ]
  },
  {
    "id": "typescript-expert",
    "name": "TypeScript 专家",
    "description": "严格类型、泛型、类型收窄与可辨识联合",
    "version": "1.0.0",
    "author": "CodeFactory",
    "tags": ["typescript", "types"],
    "download_url": null,
    "system_prompt": "You are a TypeScript expert. Favor precise types over any; use generics, discriminated unions, and narrowing to make illegal states unrepresentable. Keep inference working rather than over-annotating. Explain tricky type errors in plain terms and give the minimal fix.",
    "slash_commands": [
      { "name": "type", "description": "为这段数据/函数设计类型", "template": "Design precise TypeScript types for: {input}" },
      { "name": "fix-types", "description": "解释并修复类型错误", "template": "Explain and fix this TypeScript type error with the minimal change: {input}" }
    ]
  },
  {
    "id": "rust-expert",
    "name": "Rust 专家",
    "description": "所有权、错误处理、async 与零成本抽象",
    "version": "1.0.0",
    "author": "CodeFactory",
    "tags": ["rust", "backend"],
    "download_url": null,
    "system_prompt": "You are a Rust expert. Write safe, idiomatic Rust: borrow over clone, Result with thiserror/anyhow over panics, iterators over manual loops. Explain ownership and lifetime errors clearly and give the minimal fix. For async, mind Send/Sync bounds and avoid blocking the runtime. Avoid unsafe unless justified.",
    "slash_commands": [
      { "name": "rust", "description": "用 idiomatic Rust 实现", "template": "Implement this in idiomatic, safe Rust: {input}" },
      { "name": "borrow", "description": "解释借用/生命周期错误", "template": "Explain this Rust borrow/lifetime error and give the minimal fix: {input}" }
    ]
  },
  {
    "id": "api-designer",
    "name": "API 设计",
    "description": "REST/OpenAPI、资源建模、版本化与错误约定",
    "version": "1.0.0",
    "author": "CodeFactory",
    "tags": ["api", "rest", "design"],
    "download_url": null,
    "system_prompt": "You are an API designer. Model resources clearly, use correct HTTP methods and status codes, and keep responses consistent (envelopes, pagination, error shape per RFC 7807). Plan for versioning and backward compatibility. Provide example requests/responses and an OpenAPI sketch when useful.",
    "slash_commands": [
      { "name": "endpoint", "description": "设计一个 REST 端点", "template": "Design a REST endpoint (method, path, request, response, errors) for: {input}" },
      { "name": "openapi", "description": "生成 OpenAPI 片段", "template": "Write an OpenAPI 3 fragment for: {input}" }
    ]
  },
  {
    "id": "tech-writer",
    "name": "技术文档",
    "description": "README、docstring、用法示例与架构决策记录",
    "version": "1.0.0",
    "author": "CodeFactory",
    "tags": ["docs", "writing"],
    "download_url": null,
    "system_prompt": "You are a technical writer for developers. Write clear, skimmable docs: what it does, why, and a copy-pasteable example first. Prefer short sentences and concrete verbs. Keep README Features/Usage in sync with the actual code; never document behavior you can't see. For decisions, use a short ADR (context, decision, consequences).",
    "slash_commands": [
      { "name": "readme", "description": "为项目/模块写 README 段落", "template": "Write a clear README section (overview + usage example) for: {input}" },
      { "name": "docstring", "description": "为函数补 docstring", "template": "Write a concise docstring (purpose, params, returns, errors) for: {input}" }
    ]
  },
  {
    "id": "git-expert",
    "name": "Git 提交规范",
    "description": "Conventional Commits、清晰的 PR 描述与安全 rebase",
    "version": "1.0.0",
    "author": "CodeFactory",
    "tags": ["git", "workflow"],
    "download_url": null,
    "system_prompt": "You are a Git workflow expert. Write Conventional Commit messages (type(scope): summary) with a body that explains why, not just what. Draft clear PR descriptions (problem, change, verification). Advise on safe rebase/squash and conflict resolution. Never suggest force-pushing a shared branch.",
    "slash_commands": [
      { "name": "commit", "description": "根据改动写提交信息", "template": "Write a Conventional Commit message for these changes: {input}" },
      { "name": "pr", "description": "写 PR 描述", "template": "Write a clear PR description (problem, change, how to verify) for: {input}" }
    ]
  }
]"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: String,
    pub tags: Vec<String>,
    pub download_url: Option<String>,
    pub system_prompt: String,
    pub slash_commands: Vec<serde_json::Value>,
    #[serde(default)]
    pub installed: bool,
}

#[tauri::command]
pub async fn fetch_marketplace_skills(
    _registry_url: Option<String>,
    app: AppHandle,
) -> Result<Vec<MarketplaceSkill>, String> {
    // Phase 0 containment: the existing remote registry has no signed envelope
    // or immutable package digest. Until that contract ships, only expose the
    // catalog embedded in the signed CodeFactory app; renderer-provided URLs
    // are intentionally ignored.
    let mut skills: Vec<MarketplaceSkill> = serde_json::from_str(BUILTIN_REGISTRY)
        .map_err(|e| format!("SKILL_BUILTIN_CATALOG_INVALID: {e}"))?;

    // Mark already-installed skills
    let installed_ids: std::collections::HashSet<String> = list_skills(app)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.id)
        .collect();

    for skill in &mut skills {
        skill.installed = installed_ids.contains(&skill.id);
    }

    Ok(skills)
}

#[tauri::command]
pub async fn install_marketplace_skill(skill_id: String) -> Result<SkillManifest, String> {
    let skill = serde_json::from_str::<Vec<MarketplaceSkill>>(BUILTIN_REGISTRY)
        .map_err(|e| format!("SKILL_BUILTIN_CATALOG_INVALID: {e}"))?
        .into_iter()
        .find(|candidate| candidate.id == skill_id)
        .ok_or_else(|| format!("SKILL_MARKETPLACE_ID_UNKNOWN: {skill_id}"))?;
    install_marketplace_skill_into(skill, &user_skills_dir())
}

fn install_marketplace_skill_into(
    skill: MarketplaceSkill,
    install_root: &Path,
) -> Result<SkillManifest, String> {
    if !is_safe_skill_id(&skill.id) {
        return Err(format!(
            "SKILL_ID_INVALID: marketplace id is not portable: {:?}",
            skill.id
        ));
    }
    // The renderer submits only its selection intent. Re-resolve all package
    // content from the backend-owned embedded catalog so forged prompt content
    // cannot cross the renderer boundary.
    let trusted: MarketplaceSkill = serde_json::from_str::<Vec<MarketplaceSkill>>(BUILTIN_REGISTRY)
        .map_err(|e| format!("SKILL_BUILTIN_CATALOG_INVALID: {e}"))?
        .into_iter()
        .find(|candidate| candidate.id == skill.id)
        .ok_or_else(|| format!("SKILL_MARKETPLACE_ID_UNKNOWN: {}", skill.id))?;
    let skill_dir = prepare_skill_dir_handle(install_root, &trusted.id)?;

    // Write system_prompt.md
    skill_dir.write_atomic("system_prompt.md", trusted.system_prompt.as_bytes())?;

    // Write slash_commands.json
    let cmds_json =
        serde_json::to_string_pretty(&trusted.slash_commands).unwrap_or_else(|_| "[]".to_string());
    skill_dir.write_atomic("slash_commands.json", cmds_json.as_bytes())?;

    let manifest = SkillManifest {
        id: trusted.id.clone(),
        name: trusted.name.clone(),
        description: trusted.description.clone(),
        version: trusted.version.clone(),
        author: trusted.author.clone(),
        tags: trusted.tags.clone(),
        enabled: false,
        path: skill_dir.path().to_string_lossy().to_string(),
        source: "user".to_string(),
        lifecycle_status: "ready".to_string(),
    };

    write_manifest_to_dir(&skill_dir, &manifest)?;
    Ok(manifest)
}

// ── Agent-facing discovery / fetch (search the registry, install from a source) ─

/// Search the registry for installable skills matching `query` (name /
/// description / id / tags; empty query returns all). App-independent.
pub async fn search_registry_skills(query: &str) -> Vec<MarketplaceSkill> {
    let all: Vec<MarketplaceSkill> = serde_json::from_str(BUILTIN_REGISTRY).unwrap_or_default();
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return all;
    }
    all.into_iter()
        .filter(|s| {
            s.name.to_lowercase().contains(&q)
                || s.description.to_lowercase().contains(&q)
                || s.id.to_lowercase().contains(&q)
                || s.tags.iter().any(|t| t.to_lowercase().contains(&q))
        })
        .collect()
}

/// Install a registry skill by id (disabled). App-independent.
async fn install_marketplace_skill_by_id(id: &str) -> Result<SkillManifest, String> {
    install_marketplace_skill(id.to_string()).await
}

/// Fetch + install skill(s) from a source, landing them DISABLED (the user
/// reviews + enables). Accepts a public HTTPS manifest JSON URL or an embedded
/// registry id. Raw local paths and Git sources are blocked during Phase 0;
/// local directories must come from the Resource Center's native source picker.
/// App-independent — backs `skill_fetch`.
pub async fn fetch_skill_from_source(source: &str) -> Result<Vec<SkillManifest>, String> {
    let s = source.trim();

    // 1) Git repo.
    if s.starts_with("http")
        && (s.contains("github.com") || s.contains("gitlab.com") || s.ends_with(".git"))
    {
        return Err(
            "SKILL_SOURCE_GIT_UNAVAILABLE_PHASE0: Git 导入将在受限下载与摘要校验接入后恢复；当前请使用公开 HTTPS manifest"
                .into(),
        );
    }

    // 2) Plain http(s) URL → a JSON manifest.
    if s.starts_with("http://") || s.starts_with("https://") {
        return install_user_skill_from_url(s).await.map(|m| vec![m]);
    }

    // 3) Local paths require a backend-issued native-picker capability. Agent
    // tools and renderer IPC cannot turn an arbitrary string into read access.
    let as_path = Path::new(s);
    if as_path.is_absolute()
        || s.starts_with('.')
        || s.starts_with('~')
        || s.contains('/')
        || s.contains('\\')
        || (s.len() >= 2 && s.as_bytes()[1] == b':')
    {
        return Err(
            "SKILL_SOURCE_HANDLE_REQUIRED: 请在资源中心使用“从本地目录导入”选择目录；Agent 不能直接读取任意本机路径"
                .to_string(),
        );
    }

    // 4) Otherwise: treat as a backend-owned embedded registry id.
    install_marketplace_skill_by_id(s).await.map(|m| vec![m])
}

// ── Self-evolution P2: skill auto-evolution ───────────────────────────────────
//
// Turn recurring TASK shapes into a skill the agent drafts for itself, stored
// DISABLED for preview-then-enable. Clustering is pure + unit-tested; the draft
// is deterministic for v1 (an LLM polish pass is a noted follow-up). A proposal
// is just a normal user skill tagged "proposed" with the rationale in its
// description — so list/enable/delete work unchanged and it can never act until
// the human enables it. See docs/self-evolution/P2-skill-auto-evolution.md.

const MIN_CLUSTER: usize = 4;
const MAX_PROPOSALS_PER_RUN: usize = 3;

/// Cluster keys the user has explicitly rejected (deleted a proposed skill
/// for), so `propose_skills_from_patterns` never re-suggests them. A proposed
/// skill's `name` is always set to its cluster key (see `write_proposal_skill`
/// / `cluster_task_intents`), so recovering the key at delete time is exact —
/// no need to invert the filesystem-slug transform.
const REJECTED_PROPOSALS_FILE: &str = ".rejected_proposals.json";

fn load_rejected_proposal_keys() -> std::collections::HashSet<String> {
    let Ok(root) = SecureDir::open_existing(&user_skills_dir()) else {
        return std::collections::HashSet::new();
    };
    load_rejected_proposal_keys_from(&root).unwrap_or_default()
}

fn load_rejected_proposal_keys_from(
    root: &SecureDir,
) -> Result<std::collections::HashSet<String>, String> {
    let Some(raw) = root.read_string_optional(REJECTED_PROPOSALS_FILE)? else {
        return Ok(std::collections::HashSet::new());
    };
    serde_json::from_str::<Vec<String>>(&raw)
        .map(|values| values.into_iter().collect())
        .map_err(|error| format!("SKILL_REJECTED_PROPOSALS_INVALID: {error}"))
}

fn record_rejected_proposal_key(key: &str) -> Result<(), String> {
    let root = SecureDir::open_or_create(&user_skills_dir())?;
    record_rejected_proposal_key_in(&root, key)
}

fn record_rejected_proposal_key_in(root: &SecureDir, key: &str) -> Result<(), String> {
    let mut keys = load_rejected_proposal_keys_from(root)?;
    if !keys.insert(key.to_string()) {
        return Ok(());
    }
    let mut values = keys.into_iter().collect::<Vec<_>>();
    values.sort();
    let json = serde_json::to_vec(&values)
        .map_err(|error| format!("SKILL_REJECTED_PROPOSALS_INVALID: {error}"))?;
    root.write_atomic(REJECTED_PROPOSALS_FILE, &json)
}

#[derive(Debug, Clone)]
pub struct TaskTitleRow {
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct SkillProposalDraft {
    pub key: String,
    pub label: String,
    pub support_count: usize,
    pub examples: Vec<String>,
}

/// Normalize a task title into a cluster key: lowercase, drop punctuation /
/// pure-number / single-char tokens (ids, paths), keep the first few keywords.
fn norm_task_title(t: &str) -> String {
    t.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .filter(|w| w.chars().count() > 1 && !w.chars().all(|c| c.is_numeric()))
        .take(6)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Cluster recurring task intents. A cluster with >= MIN_CLUSTER tasks is a
/// candidate proposal. Pure.
fn cluster_task_intents(rows: &[TaskTitleRow]) -> Vec<SkillProposalDraft> {
    use std::collections::HashMap;
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    for r in rows {
        let key = norm_task_title(&r.title);
        if key.is_empty() {
            continue;
        }
        groups
            .entry(key)
            .or_default()
            .push(r.title.trim().to_string());
    }
    let mut out: Vec<SkillProposalDraft> = groups
        .into_iter()
        .filter(|(_, v)| v.len() >= MIN_CLUSTER)
        .map(|(key, mut examples)| {
            examples.sort();
            examples.dedup();
            SkillProposalDraft {
                label: key.clone(),
                support_count: examples.len(),
                examples: examples.into_iter().take(5).collect(),
                key,
            }
        })
        .collect();
    out.sort_by(|a, b| b.support_count.cmp(&a.support_count));
    out
}

/// Drop clusters already served by an existing skill (>= 2 keyword overlap with
/// a skill name/tag) or previously rejected. Pure.
fn filter_covered(
    drafts: Vec<SkillProposalDraft>,
    existing_skill_labels: &[String],
    rejected_keys: &std::collections::HashSet<String>,
) -> Vec<SkillProposalDraft> {
    drafts
        .into_iter()
        .filter(|d| {
            if rejected_keys.contains(&d.key) {
                return false;
            }
            let words: Vec<&str> = d.key.split_whitespace().collect();
            let covered = existing_skill_labels.iter().any(|s| {
                let sl = s.to_lowercase();
                words.iter().filter(|w| sl.contains(**w)).count() >= 2
            });
            !covered
        })
        .collect()
}

/// Write one cluster as a DISABLED "proposed" user skill (deterministic draft).
fn write_proposal_skill(d: &SkillProposalDraft) -> Result<SkillManifest, String> {
    let slug: String = d
        .key
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    let id = format!(
        "proposed-{}",
        if slug.is_empty() { "task".into() } else { slug }
    );
    let skill_dir = prepare_skill_dir_handle(&user_skills_dir(), &id)?;

    let examples = d
        .examples
        .iter()
        .map(|e| format!("- {e}"))
        .collect::<Vec<_>>()
        .join("\n");
    let system_prompt = format!(
        "你是协助处理「{label}」类任务的助手。用户在本项目反复做这类事，例如：\n{examples}\n\n\
处理这类请求时，沿用过往成功的做法、保持风格一致，并主动补齐这类任务常见但容易遗漏的步骤。",
        label = d.label,
        examples = examples,
    );
    skill_dir.write_atomic("system_prompt.md", system_prompt.as_bytes())?;

    let cmd_name: String = d
        .key
        .split_whitespace()
        .next()
        .unwrap_or("task")
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    let slash = serde_json::json!([{
        "name": if cmd_name.is_empty() { "task".to_string() } else { cmd_name },
        "description": format!("处理「{}」类任务", d.label),
        "template": format!("处理这个「{}」类任务：{{input}}", d.label),
    }]);
    skill_dir.write_atomic(
        "slash_commands.json",
        serde_json::to_string_pretty(&slash)
            .unwrap_or_default()
            .as_bytes(),
    )?;

    let manifest = SkillManifest {
        id: id.clone(),
        name: d.label.clone(),
        description: format!(
            "提议（{} 次证据）：你在本项目反复做「{}」类任务。预览后可编辑、启用。",
            d.support_count, d.label
        ),
        version: "0.1.0".into(),
        author: "CodeFactory (proposed)".into(),
        tags: vec!["proposed".into()],
        enabled: false,
        path: skill_dir.path().to_string_lossy().to_string(),
        source: "user".into(),
        lifecycle_status: "ready".to_string(),
    };
    write_manifest_to_dir(&skill_dir, &manifest)?;
    Ok(manifest)
}

/// Propose skills from this project's recurring task patterns. Each proposal is
/// written DISABLED; the user previews + enables (or deletes). Idempotent: a
/// cluster already covered by an existing skill (including a prior proposal)
/// is skipped, so re-running doesn't duplicate.
#[tauri::command]
pub async fn propose_skills_from_patterns(
    cwd: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<SkillManifest>, String> {
    let pool = state.db.read().await;
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT title FROM task_runs WHERE cwd = ? ORDER BY created_at DESC LIMIT 1000",
    )
    .bind(&cwd)
    .fetch_all(&*pool)
    .await
    .map_err(|e| e.to_string())?;
    drop(pool);
    let task_rows: Vec<TaskTitleRow> = rows
        .into_iter()
        .map(|(title,)| TaskTitleRow { title })
        .collect();

    let existing = list_skills(app.clone()).await?;
    let existing_labels: Vec<String> = existing
        .iter()
        .flat_map(|s| std::iter::once(s.name.clone()).chain(s.tags.iter().cloned()))
        .collect();

    let drafts = cluster_task_intents(&task_rows);
    let drafts = filter_covered(drafts, &existing_labels, &load_rejected_proposal_keys());

    let mut created = Vec::new();
    for d in drafts.into_iter().take(MAX_PROPOSALS_PER_RUN) {
        created.push(write_proposal_skill(&d)?);
    }
    if !created.is_empty() {
        let _ = app.emit("skill_proposals_updated", &created);
    }
    Ok(created)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_safe_skill_id_blocks_path_traversal() {
        assert!(is_safe_skill_id("my-cool-skill"));
        assert!(is_safe_skill_id("skill_v2.1"));
        assert!(!is_safe_skill_id(""));
        assert!(!is_safe_skill_id("."));
        assert!(!is_safe_skill_id(".."));
        assert!(!is_safe_skill_id("../../etc/passwd"));
        assert!(!is_safe_skill_id("a/b"));
        assert!(!is_safe_skill_id("a\\b"));
        assert!(!is_safe_skill_id("/tmp/outside"));
        assert!(!is_safe_skill_id("C:\\outside"));
        assert!(!is_safe_skill_id("\\\\server\\share"));
        assert!(!is_safe_skill_id("CON"));
        assert!(!is_safe_skill_id("nul.txt"));
        assert!(!is_safe_skill_id("ends-with-dot."));
        assert!(!is_safe_skill_id("MixedCase"));
        assert!(!is_safe_skill_id(&"a".repeat(65)));
    }

    #[test]
    fn skill_path_is_resolved_only_after_id_validation() {
        let root = Path::new("/synthetic/skill-root");
        assert_eq!(
            skill_path_under(root, "safe-skill").unwrap(),
            root.join("safe-skill")
        );
        for id in ["../../victim", "/tmp/victim", "C:\\victim", "CON", "foo."] {
            assert!(skill_path_under(root, id).is_err(), "{id} must fail closed");
        }
    }

    #[test]
    fn skill_path_subprocess_probe() {
        let Some(root) = std::env::var_os("CODEFACTORY_SKILL_PROBE_ROOT") else {
            return;
        };
        for id in ["../../victim", "/tmp/victim", "C:\\victim", "CON", "foo."] {
            assert!(prepare_skill_dir(Path::new(&root), id).is_err());
            assert!(delete_user_skill_in(Path::new(&root), id).is_err());
        }
    }

    #[test]
    fn skill_path_process_sentinel_keeps_outside_tree_unchanged() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("skills");
        let victim = fixture.path().join("victim.txt");
        std::fs::write(&victim, "UNCHANGED").unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("commands::skills::tests::skill_path_subprocess_probe")
            .arg("--exact")
            .env("CODEFACTORY_SKILL_PROBE_ROOT", &root)
            .status()
            .unwrap();

        assert!(status.success());
        assert_eq!(std::fs::read_to_string(victim).unwrap(), "UNCHANGED");
    }

    #[test]
    fn external_directory_import_is_installed_disabled() {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source");
        let install_root = fixture.path().join("installed");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join("SKILL.md"),
            "---\nid: continuity-helper\nname: Continuity Helper\ndescription: synthetic\n---\n\nStay continuous.",
        )
        .unwrap();

        let imported = import_skill_directories_into(&source, &install_root).unwrap();

        assert_eq!(imported.succeeded.len(), 1);
        assert!(imported.failed.is_empty());
        assert!(!imported.succeeded[0].enabled);
        let persisted: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(install_root.join("continuity-helper/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(persisted["enabled"], false);
    }

    #[cfg(unix)]
    #[test]
    fn local_source_handle_is_single_use_and_pins_the_selected_directory() {
        use std::os::unix::fs::symlink;

        let fixture = tempfile::tempdir().unwrap();
        let selected = fixture.path().join("selected");
        let held = fixture.path().join("held");
        let outside = fixture.path().join("outside");
        let install_root = fixture.path().join("installed");
        std::fs::create_dir_all(&selected).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(
            selected.join("SKILL.md"),
            "---\nid: pinned-source\nname: Pinned Source\n---\nSELECTED BODY",
        )
        .unwrap();
        std::fs::write(
            outside.join("SKILL.md"),
            "---\nid: outside-source\nname: Outside Source\n---\nOUTSIDE BODY",
        )
        .unwrap();

        let selection =
            register_skill_source(SecureDir::open_existing(&selected).unwrap()).unwrap();
        std::fs::rename(&selected, &held).unwrap();
        symlink(&outside, &selected).unwrap();

        let source = consume_skill_source(&selection.source_handle).unwrap();
        let imported = import_skill_directories_from_handle(source, &install_root).unwrap();

        assert_eq!(imported.succeeded.len(), 1);
        assert_eq!(imported.succeeded[0].id, "pinned-source");
        assert_eq!(
            std::fs::read_to_string(install_root.join("pinned-source/system_prompt.md")).unwrap(),
            "SELECTED BODY"
        );
        assert!(!install_root.join("outside-source").exists());
        assert!(consume_skill_source(&selection.source_handle).is_err());
    }

    #[tokio::test]
    async fn agent_fetch_rejects_raw_local_paths_without_reading_them() {
        let fixture = tempfile::tempdir().unwrap();
        let sentinel = fixture.path().join("sentinel.txt");
        std::fs::write(&sentinel, "UNCHANGED").unwrap();

        let error = fetch_skill_from_source(fixture.path().to_string_lossy().as_ref())
            .await
            .unwrap_err();

        assert!(error.starts_with("SKILL_SOURCE_HANDLE_REQUIRED:"));
        assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "UNCHANGED");
    }

    #[test]
    fn directory_import_returns_successes_and_failures_in_one_result() {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source");
        let install_root = fixture.path().join("installed");
        let good = source.join("good");
        let bad = source.join("bad");
        std::fs::create_dir_all(&good).unwrap();
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(
            good.join("SKILL.md"),
            "---\nid: good-skill\nname: Good Skill\n---\nSafe body",
        )
        .unwrap();
        std::fs::write(bad.join("manifest.json"), "{not-json").unwrap();

        let result = import_skill_directories_into(&source, &install_root).unwrap();

        assert_eq!(result.succeeded.len(), 1);
        assert_eq!(result.succeeded[0].id, "good-skill");
        assert_eq!(result.failed.len(), 1);
        assert_eq!(
            Path::new(&result.failed[0].path)
                .file_name()
                .and_then(|name| name.to_str()),
            Some("bad")
        );
        assert!(result.failed[0].error.contains("解析失败"));
    }

    #[test]
    fn local_import_rejects_explicit_noncanonical_ids_instead_of_slugifying_them() {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source");
        let install_root = fixture.path().join("installed");
        let manifest_skill = source.join("manifest-skill");
        let markdown_skill = source.join("markdown-skill");
        std::fs::create_dir_all(&manifest_skill).unwrap();
        std::fs::create_dir_all(&markdown_skill).unwrap();
        std::fs::write(
            manifest_skill.join("manifest.json"),
            r#"{"id":"../../victim","name":"Bad","description":"bad","version":"1.0.0","author":"fixture"}"#,
        )
        .unwrap();
        std::fs::write(
            markdown_skill.join("SKILL.md"),
            "---\nid: /tmp/victim\nname: Bad Markdown\n---\nMUST NOT INSTALL",
        )
        .unwrap();

        let result = import_skill_directories_into(&source, &install_root).unwrap();

        assert!(result.succeeded.is_empty());
        assert_eq!(result.failed.len(), 2);
        assert!(result
            .failed
            .iter()
            .all(|failure| failure.error.starts_with("SKILL_ID_INVALID:")));
        assert!(!install_root.join("victim").exists());
        assert!(!install_root.join("tmp-victim").exists());
    }

    #[test]
    fn damaged_skill_directory_remains_visible_but_ineligible() {
        let fixture = tempfile::tempdir().unwrap();
        let damaged = fixture.path().join("damaged-skill");
        std::fs::create_dir_all(&damaged).unwrap();
        std::fs::write(damaged.join("manifest.json"), "{not-json").unwrap();
        std::fs::write(damaged.join("system_prompt.md"), "MUST NOT LOAD").unwrap();

        let projected = scan_skill_dir(&fixture.path().to_path_buf(), "user");

        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].id, "damaged-skill");
        assert_eq!(projected[0].lifecycle_status, "corrupt");
        assert!(!projected[0].enabled);
        assert!(prompts_from_skill_dir(fixture.path()).is_empty());
    }

    #[test]
    fn builtin_enabled_bit_is_not_an_unverified_release_approval() {
        let fixture = tempfile::tempdir().unwrap();
        let builtin = fixture.path().join("builtin-skill");
        std::fs::create_dir_all(&builtin).unwrap();
        std::fs::write(
            builtin.join("manifest.json"),
            r#"{"id":"builtin-skill","name":"Builtin","description":"fixture","version":"1.0.0","author":"CodeFactory","tags":[],"enabled":true}"#,
        )
        .unwrap();
        std::fs::write(builtin.join("system_prompt.md"), "MUST REQUIRE REVIEW").unwrap();

        let projected = scan_skill_dir(&fixture.path().to_path_buf(), "builtin");

        assert_eq!(projected.len(), 1);
        assert!(!projected[0].enabled);
    }

    #[test]
    fn explicit_remote_skill_sources_require_public_https() {
        assert!(validate_explicit_skill_url("https://example.com/skill.json").is_ok());
        for source in [
            "http://example.com/skill.json",
            "https://localhost/skill.json",
            "https://127.0.0.1/skill.json",
            "https://10.0.0.1/skill.json",
            "https://100.64.0.1/skill.json",
            "https://169.254.169.254/latest/meta-data",
            "https://198.18.0.1/skill.json",
            "https://192.0.2.1/skill.json",
            "https://203.0.113.1/skill.json",
            "https://[::1]/skill.json",
            "https://[2001:db8::1]/skill.json",
            "https://[2606:4700:4700::1111]/skill.json",
            "https://[::ffff:127.0.0.1]/skill.json",
            "https://user:secret@example.com/skill.json",
            "https://example.com/skill.json?token=secret",
            "https://example.com/skill.json#fragment",
        ] {
            assert!(
                validate_explicit_skill_url(source).is_err(),
                "{source} must fail before network access"
            );
        }
    }

    #[test]
    fn marketplace_install_uses_backend_catalog_not_renderer_content() {
        let fixture = tempfile::tempdir().unwrap();
        let forged = MarketplaceSkill {
            id: "python-expert".into(),
            name: "Forged".into(),
            description: "Forged".into(),
            version: "999.0.0".into(),
            author: "attacker".into(),
            tags: vec![],
            download_url: None,
            system_prompt: "PWNED RENDERER CONTENT".into(),
            slash_commands: vec![],
            installed: false,
        };

        let installed = install_marketplace_skill_into(forged, fixture.path()).unwrap();
        let prompt =
            std::fs::read_to_string(fixture.path().join("python-expert/system_prompt.md")).unwrap();

        assert_eq!(installed.name, "Python 专家");
        assert!(!installed.enabled);
        assert!(!prompt.contains("PWNED"));
        assert!(prompt.contains("expert Python developer"));
    }

    #[test]
    fn marketplace_install_cannot_replace_an_existing_enabled_skill() {
        let fixture = tempfile::tempdir().unwrap();
        let existing = fixture.path().join("python-expert");
        std::fs::create_dir_all(&existing).unwrap();
        let old_manifest = r#"{"id":"python-expert","name":"Old","description":"old","version":"1.0.0","author":"fixture","tags":[],"enabled":true}"#;
        std::fs::write(existing.join("manifest.json"), old_manifest).unwrap();
        std::fs::write(existing.join("system_prompt.md"), "OLD REVIEWED CONTENT").unwrap();
        let selected: MarketplaceSkill =
            serde_json::from_str::<Vec<MarketplaceSkill>>(BUILTIN_REGISTRY)
                .unwrap()
                .into_iter()
                .find(|skill| skill.id == "python-expert")
                .unwrap();

        let error = install_marketplace_skill_into(selected, fixture.path()).unwrap_err();

        assert!(error.contains("SKILL_ID_ALREADY_INSTALLED"));
        assert_eq!(
            std::fs::read_to_string(existing.join("system_prompt.md")).unwrap(),
            "OLD REVIEWED CONTENT"
        );
        assert_eq!(
            std::fs::read_to_string(existing.join("manifest.json")).unwrap(),
            old_manifest
        );
    }

    #[test]
    fn updating_an_enabled_skill_disables_it_before_replacing_content() {
        let fixture = tempfile::tempdir().unwrap();
        let skill = fixture.path().join("review-again");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("manifest.json"),
            r#"{"id":"review-again","name":"Review Again","description":"old","version":"1.0.0","author":"fixture","tags":[],"enabled":true}"#,
        )
        .unwrap();
        std::fs::write(skill.join("system_prompt.md"), "old").unwrap();

        let updated = update_user_skill_in(
            fixture.path(),
            "review-again",
            None,
            Some("new"),
            Some("new prompt"),
        )
        .unwrap();

        assert!(!updated.enabled);
        assert_eq!(
            std::fs::read_to_string(skill.join("system_prompt.md")).unwrap(),
            "new prompt"
        );
        let persisted: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(skill.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(persisted["enabled"], false);
    }

    #[cfg(unix)]
    #[test]
    fn install_target_rejects_a_symlink_before_writing() {
        use std::os::unix::fs::symlink;

        let fixture = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), fixture.path().join("linked-skill")).unwrap();

        assert!(prepare_skill_dir(fixture.path(), "linked-skill").is_err());
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn local_import_does_not_follow_source_directory_or_file_symlinks() {
        use std::os::unix::fs::symlink;

        let fixture = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let install_root = fixture.path().join("installed");
        std::fs::write(
            outside.path().join("SKILL.md"),
            "---\nid: outside-skill\nname: Outside\n---\nPWNED",
        )
        .unwrap();
        symlink(outside.path(), fixture.path().join("linked-directory")).unwrap();

        let linked_file_source = fixture.path().join("linked-file-source");
        std::fs::create_dir_all(&linked_file_source).unwrap();
        symlink(
            outside.path().join("SKILL.md"),
            linked_file_source.join("SKILL.md"),
        )
        .unwrap();

        assert!(import_skill_directories_into(fixture.path(), &install_root).is_err());
        assert!(!install_root.join("outside-skill").exists());
    }

    #[cfg(unix)]
    #[test]
    fn package_file_rejects_a_symlink_before_writing() {
        use std::os::unix::fs::symlink;

        let fixture = tempfile::tempdir().unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        let skill = fixture.path().join("linked-file-skill");
        std::fs::create_dir_all(&skill).unwrap();
        symlink(outside.path(), skill.join("system_prompt.md")).unwrap();

        let skill_dir = SecureDir::open_existing(&skill).unwrap();
        assert!(skill_dir.read_optional("system_prompt.md").is_err());
        assert_eq!(std::fs::read_to_string(outside.path()).unwrap(), "");
    }

    #[cfg(unix)]
    #[test]
    fn rejected_proposal_state_never_follows_symlinks_or_hardlinks() {
        use std::os::unix::fs::symlink;

        for link_kind in ["symlink", "hardlink"] {
            let fixture = tempfile::tempdir().unwrap();
            let root_path = fixture.path().join("skills");
            let outside = fixture.path().join("outside.json");
            std::fs::create_dir(&root_path).unwrap();
            std::fs::write(&outside, "[\"OUTSIDE\"]").unwrap();
            let state_path = root_path.join(REJECTED_PROPOSALS_FILE);
            if link_kind == "symlink" {
                symlink(&outside, &state_path).unwrap();
            } else {
                std::fs::hard_link(&outside, &state_path).unwrap();
            }
            let root = SecureDir::open_existing(&root_path).unwrap();

            let error = record_rejected_proposal_key_in(&root, "should-not-write").unwrap_err();

            assert!(error.starts_with("SKILL_PATH_UNSAFE:"));
            assert_eq!(std::fs::read_to_string(&outside).unwrap(), "[\"OUTSIDE\"]");
        }
    }

    #[test]
    fn headless_skill_prompts_require_current_external_review_receipt() {
        let dir = std::env::temp_dir().join(format!("cf-headless-skills-{}", std::process::id()));
        let review_dir =
            std::env::temp_dir().join(format!("cf-headless-skill-reviews-{}", std::process::id()));
        let on = dir.join("on-skill");
        std::fs::create_dir_all(&on).unwrap();
        std::fs::create_dir_all(&review_dir).unwrap();
        std::fs::write(
            on.join("manifest.json"),
            r#"{"id":"on-skill","name":"On","description":"d","version":"1.0.0","author":"a","tags":[],"enabled":true}"#,
        )
        .unwrap();
        std::fs::write(on.join("system_prompt.md"), "  ENABLED PROMPT BODY  ").unwrap();

        let off = dir.join("off-skill");
        std::fs::create_dir_all(&off).unwrap();
        std::fs::write(
            off.join("manifest.json"),
            r#"{"id":"off-skill","name":"Off","description":"d","version":"1.0.0","author":"a","tags":[],"enabled":false}"#,
        )
        .unwrap();
        std::fs::write(off.join("system_prompt.md"), "DISABLED PROMPT").unwrap();

        assert!(prompts_from_skill_dir_with_reviews(&dir, Some(&review_dir)).is_empty());
        let review =
            activation_review_fingerprint(&on, "on-skill", "On", "d", "1.0.0", "a", &[]).unwrap();
        // A legacy package-local marker is untrusted even if it contains the
        // exact public fingerprint.
        std::fs::write(on.join(".activation-reviewed-v1"), &review).unwrap();
        assert!(prompts_from_skill_dir_with_reviews(&dir, Some(&review_dir)).is_empty());
        std::fs::write(
            review_dir.join(activation_review_filename("on-skill").unwrap()),
            review,
        )
        .unwrap();
        let prompts = prompts_from_skill_dir_with_reviews(&dir, Some(&review_dir));
        assert_eq!(prompts.len(), 1);
        let installed_root = on.canonicalize().unwrap();
        assert!(prompts[0].starts_with("<enabled_skill>"));
        assert!(prompts[0].contains("skill_id: on-skill"));
        assert!(prompts[0].contains(&format!("skill_root: {}", installed_root.display())));
        assert!(prompts[0]
            .contains("Resolve scripts/, references/, and assets/ relative to skill_root"));
        assert!(prompts[0].ends_with("ENABLED PROMPT BODY\n</enabled_skill>"));

        std::fs::write(on.join("system_prompt.md"), "CHANGED AFTER REVIEW").unwrap();
        assert!(prompts_from_skill_dir_with_reviews(&dir, Some(&review_dir)).is_empty());

        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("commands::skills::tests::headless_content_drift_restart_probe")
            .arg("--exact")
            .env("CODEFACTORY_SKILL_RESTART_PROBE_ROOT", &dir)
            .env("CODEFACTORY_SKILL_RESTART_PROBE_REVIEW_ROOT", &review_dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "fresh headless process loaded drifted content: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // Missing dir → empty, never a panic.
        assert!(prompts_from_skill_dir_with_reviews(
            std::path::Path::new("/definitely/not/here"),
            Some(&review_dir)
        )
        .is_empty());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&review_dir);
    }

    #[test]
    fn enable_requires_the_exact_fingerprint_that_was_displayed() {
        let fixture = tempfile::tempdir().unwrap();
        let skill_path = fixture.path().join("installed/review-cas");
        let review_root = fixture.path().join("reviews");
        std::fs::create_dir_all(&skill_path).unwrap();
        std::fs::write(skill_path.join("system_prompt.md"), "DISPLAYED").unwrap();
        let mut manifest = SkillManifest {
            id: "review-cas".into(),
            name: "Review CAS".into(),
            description: "fixture".into(),
            version: "1.0.0".into(),
            author: "fixture".into(),
            tags: vec![],
            enabled: false,
            path: skill_path.to_string_lossy().to_string(),
            source: "user".into(),
            lifecycle_status: "ready".into(),
        };
        let displayed = activation_review_fingerprint(
            &skill_path,
            &manifest.id,
            &manifest.name,
            &manifest.description,
            &manifest.version,
            &manifest.author,
            &manifest.tags,
        )
        .unwrap();
        std::fs::write(skill_path.join("system_prompt.md"), "CHANGED").unwrap();
        let skill_dir = SecureDir::open_existing(&skill_path).unwrap();

        let error =
            enable_user_skill_in(&skill_dir, &review_root, &mut manifest, &displayed).unwrap_err();

        assert!(error.starts_with("SKILL_REVIEW_CONTENT_CHANGED:"));
        assert!(!manifest.enabled);
        assert!(!review_root.exists());
    }

    #[test]
    fn disable_revokes_review_so_manifest_tampering_cannot_reactivate_after_restart() {
        let fixture = tempfile::tempdir().unwrap();
        let install_root = fixture.path().join("installed");
        let skill_path = install_root.join("disable-revokes");
        let review_root = fixture.path().join("reviews");
        std::fs::create_dir_all(&skill_path).unwrap();
        std::fs::create_dir_all(&review_root).unwrap();
        std::fs::write(skill_path.join("system_prompt.md"), "REVIEWED BODY").unwrap();
        let mut manifest = SkillManifest {
            id: "disable-revokes".into(),
            name: "Disable Revokes".into(),
            description: "fixture".into(),
            version: "1.0.0".into(),
            author: "fixture".into(),
            tags: vec![],
            enabled: true,
            path: skill_path.to_string_lossy().to_string(),
            source: "user".into(),
            lifecycle_status: "ready".into(),
        };
        let review = activation_review_fingerprint(
            &skill_path,
            &manifest.id,
            &manifest.name,
            &manifest.description,
            &manifest.version,
            &manifest.author,
            &manifest.tags,
        )
        .unwrap();
        std::fs::write(
            review_root.join(activation_review_filename(&manifest.id).unwrap()),
            review,
        )
        .unwrap();
        let skill_dir = SecureDir::open_existing(&skill_path).unwrap();
        write_manifest_to_dir(&skill_dir, &manifest).unwrap();

        disable_user_skill_in(&skill_dir, &review_root, &mut manifest).unwrap();
        manifest.enabled = true;
        write_manifest_to_dir(&skill_dir, &manifest).unwrap();

        assert!(prompts_from_skill_dir_with_reviews(&install_root, Some(&review_root)).is_empty());
        assert!(!review_root
            .join(activation_review_filename(&manifest.id).unwrap())
            .exists());
    }

    #[test]
    fn headless_content_drift_restart_probe() {
        let Some(root) = std::env::var_os("CODEFACTORY_SKILL_RESTART_PROBE_ROOT") else {
            return;
        };
        let reviews = std::env::var_os("CODEFACTORY_SKILL_RESTART_PROBE_REVIEW_ROOT").unwrap();
        assert!(
            prompts_from_skill_dir_with_reviews(Path::new(&root), Some(Path::new(&reviews)))
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn skill_bundle_copy_preserves_nested_resources() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let destination = root.path().join("installed");
        std::fs::create_dir_all(source.join("scripts/nested")).unwrap();
        std::fs::create_dir_all(source.join("references")).unwrap();
        std::fs::create_dir_all(source.join("assets")).unwrap();
        std::fs::create_dir_all(source.join(".git/objects")).unwrap();
        std::fs::write(source.join("SKILL.md"), "# Fixture\nbody").unwrap();
        std::fs::write(
            source.join("scripts/nested/run.sh"),
            "#!/bin/sh\necho SKILL_OK\n",
        )
        .unwrap();
        std::fs::write(source.join("references/usage.md"), "usage").unwrap();
        std::fs::write(source.join("assets/payload.txt"), "asset").unwrap();
        std::fs::write(source.join(".git/objects/secret"), "git internals").unwrap();
        std::fs::create_dir_all(&destination).unwrap();
        let source_dir = SecureDir::open_existing(&source).unwrap();
        let destination_dir = SecureDir::open_existing(&destination).unwrap();
        copy_skill_payload(&source_dir, &destination_dir).expect("safe bundle copy");
        std::fs::remove_dir_all(&source).unwrap();

        assert_eq!(
            std::fs::read_to_string(destination.join("scripts/nested/run.sh")).unwrap(),
            "#!/bin/sh\necho SKILL_OK\n"
        );
        assert_eq!(
            std::fs::read_to_string(destination.join("references/usage.md")).unwrap(),
            "usage"
        );
        assert_eq!(
            std::fs::read_to_string(destination.join("assets/payload.txt")).unwrap(),
            "asset"
        );
        assert!(!destination.join(".git").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_skill_payload_fails_closed_without_replacing_existing_install() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let installed = root.path().join("installed-skills");
        let existing = installed.join("linked-skill");
        std::fs::create_dir_all(&existing).unwrap();
        std::fs::write(existing.join("manifest.json"), "existing-manifest").unwrap();
        std::fs::create_dir_all(source.join("assets")).unwrap();
        std::fs::write(
            source.join("SKILL.md"),
            "---\nname: Linked Skill\ndescription: fixture\n---\n\nbody",
        )
        .unwrap();
        symlink("/etc/passwd", source.join("assets/escape")).unwrap();

        let source_dir = SecureDir::open_existing(&source).unwrap();
        let error = import_one_skill_dir_into(&source_dir, &installed).unwrap_err();

        assert!(error.starts_with("SKILL_PATH_UNSAFE:"), "{error}");
        assert_eq!(
            std::fs::read_to_string(existing.join("manifest.json")).unwrap(),
            "existing-manifest"
        );
    }

    #[test]
    fn local_directory_install_survives_source_cleanup_with_nested_resources() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("cloned-source/portable-skill");
        let installed = root.path().join("installed-skills");
        std::fs::create_dir_all(source.join("scripts")).unwrap();
        std::fs::create_dir_all(source.join("references")).unwrap();
        std::fs::write(
            source.join("SKILL.md"),
            "---\nname: Portable Skill\ndescription: fixture\n---\n\nUse scripts/run.sh.",
        )
        .unwrap();
        std::fs::write(source.join("scripts/run.sh"), "#!/bin/sh\necho SKILL_OK\n").unwrap();
        std::fs::write(source.join("references/usage.md"), "run usage").unwrap();

        let source_dir = SecureDir::open_existing(&source).unwrap();
        let mut manifest = import_one_skill_dir_into(&source_dir, &installed).unwrap();
        std::fs::remove_dir_all(root.path().join("cloned-source")).unwrap();

        let installed_root = PathBuf::from(&manifest.path);
        assert!(!manifest.enabled);
        assert_eq!(
            std::fs::read_to_string(installed_root.join("scripts/run.sh")).unwrap(),
            "#!/bin/sh\necho SKILL_OK\n"
        );
        assert_eq!(
            std::fs::read_to_string(installed_root.join("references/usage.md")).unwrap(),
            "run usage"
        );
        let review_root = root.path().join("reviews");
        let installed_dir = SecureDir::open_existing(&installed_root).unwrap();
        let fingerprint = activation_review_fingerprint(
            &installed_root,
            &manifest.id,
            &manifest.name,
            &manifest.description,
            &manifest.version,
            &manifest.author,
            &manifest.tags,
        )
        .unwrap();
        enable_user_skill_in(&installed_dir, &review_root, &mut manifest, &fingerprint).unwrap();
        let context = prompts_from_skill_dir_with_reviews(&installed, Some(&review_root))
            .into_iter()
            .next()
            .unwrap();
        assert!(context.contains(&format!(
            "skill_root: {}",
            installed_root.canonicalize().unwrap().display()
        )));
        assert!(!context.contains("cloned-source"));
    }

    #[test]
    fn over_limit_bundle_fails_closed_without_replacing_existing_install() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let installed = root.path().join("installed-skills");
        let existing = installed.join("too-deep");
        std::fs::create_dir_all(&existing).unwrap();
        std::fs::write(existing.join("manifest.json"), "existing-manifest").unwrap();
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(
            source.join("SKILL.md"),
            "---\nname: Too Deep\ndescription: fixture\n---\n\nbody",
        )
        .unwrap();
        let mut deep = source.clone();
        for index in 0..=MAX_SKILL_PAYLOAD_DEPTH {
            deep = deep.join(format!("d{index}"));
        }
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("payload.txt"), "too deep").unwrap();

        let source_dir = SecureDir::open_existing(&source).unwrap();
        let error = import_one_skill_dir_into(&source_dir, &installed).unwrap_err();

        assert!(error.contains("安全目录深度"), "{error}");
        assert_eq!(
            std::fs::read_to_string(existing.join("manifest.json")).unwrap(),
            "existing-manifest"
        );
        let names: Vec<_> = std::fs::read_dir(&installed)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["too-deep"]);
    }

    #[test]
    fn scan_skill_roots_previews_openclaw_skills_and_marks_installed() {
        // One-click OpenClaw import (WorkBuddy-gap P2): scan known roots,
        // preview name/description, and mark skills whose id already exists
        // locally so the UI can grey them out instead of double-importing.
        let root = std::env::temp_dir().join(format!("cf-openclaw-scan-{}", std::process::id()));
        let skill = root.join("dream-governor");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: dream-governor\ndescription: Turn dreams into governance work\n---\n\n# Dream Governor\nbody",
        )
        .unwrap();
        let empty = root.join("not-a-skill");
        std::fs::create_dir_all(&empty).unwrap();

        let found = scan_skill_roots(&[root.clone()], &["dream-governor".to_string()]);
        assert_eq!(found.len(), 1);
        let preview = &found[0];
        assert_eq!(preview.name, "dream-governor");
        assert!(preview.description.contains("governance work"));
        assert!(preview.already_installed);
        assert!(preview.path.ends_with("dream-governor"));

        let fresh = scan_skill_roots(&[root.clone()], &[]);
        assert!(!fresh[0].already_installed);

        // Missing roots scan to empty, never error.
        let ghost = std::path::PathBuf::from("/definitely/not/a/real/root");
        assert!(scan_skill_roots(&[ghost], &[]).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Real-machine smoke: when actual OpenClaw skills exist on this
    /// machine, the scanner must parse them. Skips silently elsewhere.
    #[test]
    fn scan_finds_real_openclaw_skills_when_present() {
        let roots = openclaw_skill_roots();
        if !roots.iter().any(|r| r.is_dir()) {
            eprintln!("skipping real openclaw scan smoke: no roots on this machine");
            return;
        }
        let found = scan_skill_roots(&roots, &[]);
        for preview in &found {
            assert!(!preview.name.trim().is_empty());
            assert!(!preview.path.trim().is_empty());
        }
    }

    #[test]
    fn openclaw_skill_roots_cover_the_known_locations() {
        let roots = openclaw_skill_roots();
        let joined = roots
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect::<Vec<_>>()
            .join(";");
        // Normalize separators so the assertion holds on Windows (backslashes)
        // as well as unix — the roots themselves are platform-native paths.
        assert!(joined.contains(".openclaw/workspace/skills"));
        assert!(joined.contains(".claude/skills"));
    }

    #[test]
    fn builtin_registry_parses_into_unique_usable_skills() {
        let skills: Vec<MarketplaceSkill> = serde_json::from_str(BUILTIN_REGISTRY)
            .expect("BUILTIN_REGISTRY must be valid JSON parseable into MarketplaceSkill");

        // 3 original starters + 10 curated developer skills.
        assert_eq!(skills.len(), 13, "expected 13 builtin marketplace skills");

        // Ids must be unique (install routes by id).
        let mut ids: Vec<&str> = skills.iter().map(|s| s.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), skills.len(), "skill ids must be unique");

        // Every skill must ship a non-empty prompt + at least one slash command,
        // so it actually does something once installed.
        for s in &skills {
            assert!(
                !s.system_prompt.trim().is_empty(),
                "{} has an empty system_prompt",
                s.id
            );
            assert!(!s.name.trim().is_empty(), "{} has an empty name", s.id);
            assert!(
                !s.slash_commands.is_empty(),
                "{} has no slash commands",
                s.id
            );
        }
    }

    fn tr(title: &str) -> TaskTitleRow {
        TaskTitleRow {
            title: title.into(),
        }
    }

    #[test]
    fn norm_task_title_keeps_keywords_drops_ids() {
        assert_eq!(
            norm_task_title("Write Release PR #128 for /a/b"),
            "write release pr for"
        );
        assert_eq!(
            norm_task_title("  Add   Tauri command  "),
            "add tauri command"
        );
        assert_eq!(norm_task_title("123 / 456"), "");
    }

    #[test]
    fn cluster_task_intents_needs_min_cluster() {
        let rows = vec![
            tr("Write release PR description and notes for v1"),
            tr("write release pr description and notes for v2"),
            tr("Write Release PR Description And Notes (v3)"),
            tr("write release PR description and notes — v4"),
            // unrelated one-offs → no cluster
            tr("fix login bug"),
            tr("fix a different thing"),
        ];
        let out = cluster_task_intents(&rows);
        assert_eq!(out.len(), 1, "only the 4x recurring intent clusters");
        assert_eq!(out[0].support_count, 4);
        assert!(out[0].key.contains("release"));
    }

    #[test]
    fn filter_covered_drops_existing_and_rejected() {
        let drafts = vec![
            SkillProposalDraft {
                key: "write release pr".into(),
                label: "write release pr".into(),
                support_count: 5,
                examples: vec![],
            },
            SkillProposalDraft {
                key: "add tauri command".into(),
                label: "add tauri command".into(),
                support_count: 4,
                examples: vec![],
            },
        ];
        // An existing skill "Release PR helper" covers the first (overlap: release, pr).
        let existing = vec!["Release PR helper".to_string()];
        let mut rejected = std::collections::HashSet::new();
        rejected.insert("add tauri command".to_string());
        let out = filter_covered(drafts, &existing, &rejected);
        assert!(out.is_empty(), "one covered, one rejected → both filtered");
    }
}
