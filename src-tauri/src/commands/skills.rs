// SPDX-License-Identifier: Apache-2.0
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

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
}

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

// ── Scan a directory for skill manifests ──────────────────────────────────────

fn scan_skill_dir(dir: &PathBuf, source: &str) -> Vec<SkillManifest> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };

    let mut skills = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join("manifest.json");
        let Ok(raw) = std::fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(mf) = serde_json::from_str::<ManifestFile>(&raw) else {
            continue;
        };
        skills.push(SkillManifest {
            id: mf.id,
            name: mf.name,
            description: mf.description,
            version: mf.version,
            author: mf.author,
            tags: mf.tags,
            enabled: mf.enabled,
            path: path.to_string_lossy().to_string(),
            source: source.to_string(),
        });
    }
    skills
}

// ── Write manifest back to disk ───────────────────────────────────────────────

fn write_manifest(skill_path: &str, manifest: &SkillManifest) -> Result<(), String> {
    let mf = serde_json::json!({
        "id": manifest.id,
        "name": manifest.name,
        "description": manifest.description,
        "version": manifest.version,
        "author": manifest.author,
        "tags": manifest.tags,
        "enabled": manifest.enabled,
    });
    let path = PathBuf::from(skill_path).join("manifest.json");
    std::fs::write(path, serde_json::to_string_pretty(&mf).unwrap_or_default())
        .map_err(|e| e.to_string())
}

/// Copy a builtin skill directory to the user dir so we can mutate it.
fn copy_to_user_dir(skill_path: &str) -> Result<String, String> {
    let src = PathBuf::from(skill_path);
    let dest = user_skills_dir().join(src.file_name().unwrap_or_default());
    if dest.exists() {
        // Already copied
        return Ok(dest.to_string_lossy().to_string());
    }
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(&src).map_err(|e| e.to_string())?.flatten() {
        let dest_file = dest.join(entry.file_name());
        std::fs::copy(entry.path(), dest_file).map_err(|e| e.to_string())?;
    }
    Ok(dest.to_string_lossy().to_string())
}

// ── Commands ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_skills(app: AppHandle) -> Result<Vec<SkillManifest>, String> {
    let mut map: std::collections::HashMap<String, SkillManifest> = std::collections::HashMap::new();

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

    let skill_path = PathBuf::from(&manifest.path);

    let system_prompt = std::fs::read_to_string(skill_path.join("system_prompt.md"))
        .unwrap_or_default();

    let slash_commands: Vec<SlashCommand> = std::fs::read_to_string(skill_path.join("slash_commands.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let has_tool_policy = skill_path.join("tool_policy.json").exists();

    Ok(SkillDetail {
        manifest,
        system_prompt,
        slash_commands,
        has_tool_policy,
    })
}

#[tauri::command]
pub async fn enable_skill(id: String, app: AppHandle) -> Result<(), String> {
    let skills = list_skills(app.clone()).await?;
    let mut manifest = skills
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("Skill '{id}' not found"))?;

    // If builtin, copy to user dir first
    let skill_path = if manifest.source == "builtin" {
        let new_path = copy_to_user_dir(&manifest.path)?;
        manifest.path = new_path.clone();
        manifest.source = "user".to_string();
        new_path
    } else {
        manifest.path.clone()
    };

    manifest.enabled = true;
    write_manifest(&skill_path, &manifest)
}

#[tauri::command]
pub async fn disable_skill(id: String, app: AppHandle) -> Result<(), String> {
    let skills = list_skills(app.clone()).await?;
    let mut manifest = skills
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("Skill '{id}' not found"))?;

    // If builtin, copy to user dir first
    let skill_path = if manifest.source == "builtin" {
        let new_path = copy_to_user_dir(&manifest.path)?;
        manifest.path = new_path.clone();
        manifest.source = "user".to_string();
        new_path
    } else {
        manifest.path.clone()
    };

    manifest.enabled = false;
    write_manifest(&skill_path, &manifest)
}

#[tauri::command]
pub async fn install_skill_from_url(url: String, _app: AppHandle) -> Result<SkillManifest, String> {
    // Fetch the JSON manifest (with optional embedded system_prompt)
    let response = reqwest::get(&url)
        .await
        .map_err(|e| format!("Failed to fetch: {e}"))?;
    let raw = response
        .text()
        .await
        .map_err(|e| format!("Failed to read response: {e}"))?;

    let mf: ManifestFile =
        serde_json::from_str(&raw).map_err(|e| format!("Invalid manifest JSON: {e}"))?;

    let user_dir = user_skills_dir();
    let skill_dir = user_dir.join(&mf.id);
    std::fs::create_dir_all(&skill_dir).map_err(|e| e.to_string())?;

    // Write system_prompt.md
    let system_prompt = mf.system_prompt.clone().unwrap_or_default();
    std::fs::write(skill_dir.join("system_prompt.md"), &system_prompt)
        .map_err(|e| e.to_string())?;

    let manifest = SkillManifest {
        id: mf.id.clone(),
        name: mf.name.clone(),
        description: mf.description.clone(),
        version: mf.version.clone(),
        author: mf.author.clone(),
        tags: mf.tags.clone(),
        enabled: false,
        path: skill_dir.to_string_lossy().to_string(),
        source: "user".to_string(),
    };

    write_manifest(&manifest.path, &manifest)?;
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
    ParsedSkillMd { id, name, description, tags, body }
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

fn is_skill_dir(d: &Path) -> bool {
    d.join("SKILL.md").exists() || d.join("manifest.json").exists()
}

/// Walk `dir` (bounded depth) collecting every skill directory — one holding a
/// SKILL.md or manifest.json. A skill dir is a leaf (we don't descend into it),
/// so a single skill, a `skills/<name>/` collection, or a whole repo all work.
/// Hidden dirs (.git, …) are skipped.
fn collect_skill_dirs(dir: &Path, depth: u8, out: &mut Vec<PathBuf>) {
    if is_skill_dir(dir) {
        out.push(dir.to_path_buf());
        return;
    }
    if depth == 0 {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            let hidden = p
                .file_name()
                .map(|n| n.to_string_lossy().starts_with('.'))
                .unwrap_or(false);
            if p.is_dir() && !hidden {
                collect_skill_dirs(&p, depth - 1, out);
            }
        }
    }
}

/// Import one skill directory into the user skills folder. Returns its manifest.
fn import_one_skill_dir(src: &Path) -> Result<SkillManifest, String> {
    let (id_raw, name, description, version, author, tags, body): (
        String,
        String,
        String,
        String,
        String,
        Vec<String>,
        Option<String>,
    ) = if src.join("manifest.json").exists() {
        let raw =
            std::fs::read_to_string(src.join("manifest.json")).map_err(|e| e.to_string())?;
        let mf: ManifestFile =
            serde_json::from_str(&raw).map_err(|e| format!("manifest.json 解析失败: {e}"))?;
        (mf.id, mf.name, mf.description, mf.version, mf.author, mf.tags, mf.system_prompt)
    } else {
        let raw = std::fs::read_to_string(src.join("SKILL.md")).map_err(|e| e.to_string())?;
        let p = parse_skill_md(&raw);
        (p.id, p.name, p.description, "1.0.0".into(), "imported".into(), p.tags, Some(p.body))
    };

    let dir_name = src
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let id = if !id_raw.trim().is_empty() {
        slugify(&id_raw)
    } else if !name.trim().is_empty() {
        slugify(&name)
    } else {
        slugify(&dir_name)
    };

    let dest = user_skills_dir().join(&id);
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;

    let manifest = SkillManifest {
        id: id.clone(),
        name: if name.trim().is_empty() { id.clone() } else { name },
        description,
        version,
        author,
        tags,
        enabled: true, // imported skills are active immediately
        path: dest.to_string_lossy().to_string(),
        source: "user".to_string(),
    };
    write_manifest(&manifest.path, &manifest)?;

    // system_prompt.md: SKILL.md body wins; else copy an existing one if present.
    if let Some(body) = body {
        std::fs::write(dest.join("system_prompt.md"), body).map_err(|e| e.to_string())?;
    } else if src.join("system_prompt.md").exists() {
        let _ = std::fs::copy(src.join("system_prompt.md"), dest.join("system_prompt.md"));
    }
    // Carry across the optional extras CodeFactory understands.
    for f in ["slash_commands.json", "tool_policy.json"] {
        if src.join(f).exists() {
            let _ = std::fs::copy(src.join(f), dest.join(f));
        }
    }
    Ok(manifest)
}

/// Import skill(s) from a local directory — a single skill folder, a
/// `skills/<name>/` collection, or a whole repo. Parses `SKILL.md` frontmatter
/// (superpowers / openspec format) when there's no `manifest.json`.
#[tauri::command]
pub async fn install_skill_from_directory(dir_path: String) -> Result<Vec<SkillManifest>, String> {
    let src = PathBuf::from(&dir_path);
    if !src.is_dir() {
        return Err("不是有效的目录".to_string());
    }
    let mut dirs = Vec::new();
    collect_skill_dirs(&src, 3, &mut dirs);
    if dirs.is_empty() {
        return Err("目录里没找到 skill（需要含 SKILL.md 或 manifest.json 的文件夹）".to_string());
    }
    let mut imported = Vec::new();
    for d in dirs {
        match import_one_skill_dir(&d) {
            Ok(m) => imported.push(m),
            Err(e) => tracing::warn!("skill 导入跳过 {}: {e}", d.display()),
        }
    }
    if imported.is_empty() {
        return Err("找到了 skill 目录但全部导入失败".to_string());
    }
    Ok(imported)
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

    std::fs::remove_dir_all(&manifest.path).map_err(|e| e.to_string())
}

/// Returns the combined system prompt: base + all enabled skills.
/// Used by the agent loop.
pub async fn get_active_system_prompt(base: &str, app: &AppHandle) -> String {
    let skills = match list_skills(app.clone()).await {
        Ok(s) => s,
        Err(_) => return base.to_string(),
    };

    let enabled_prompts: Vec<String> = skills
        .iter()
        .filter(|s| s.enabled)
        .filter_map(|s| {
            let path = PathBuf::from(&s.path).join("system_prompt.md");
            std::fs::read_to_string(path).ok()
        })
        .collect();

    if enabled_prompts.is_empty() {
        return base.to_string();
    }

    let mut result = base.to_string();
    for prompt in enabled_prompts {
        let trimmed = prompt.trim();
        if !trimmed.is_empty() {
            result.push_str("\n\n---\n\n");
            result.push_str(trimmed);
        }
    }
    result
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
    registry_url: Option<String>,
    app: AppHandle,
) -> Result<Vec<MarketplaceSkill>, String> {
    // Try fetching from URL first, fall back to builtin
    let (raw, from_remote) = if let Some(url) = registry_url {
        match reqwest::get(&url).await {
            Ok(resp) => match resp.text().await {
                Ok(text) => (text, true),
                Err(_) => (BUILTIN_REGISTRY.to_string(), false),
            },
            Err(_) => (BUILTIN_REGISTRY.to_string(), false),
        }
    } else {
        (BUILTIN_REGISTRY.to_string(), false)
    };

    let _ = from_remote; // used by frontend via the "local_catalog" flag on each item

    let mut skills: Vec<MarketplaceSkill> =
        serde_json::from_str(&raw).map_err(|e| format!("Failed to parse registry: {e}"))?;

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
pub async fn install_marketplace_skill(
    skill: MarketplaceSkill,
) -> Result<SkillManifest, String> {
    let user_dir = user_skills_dir();
    let skill_dir = user_dir.join(&skill.id);
    std::fs::create_dir_all(&skill_dir).map_err(|e| e.to_string())?;

    // Write system_prompt.md
    std::fs::write(skill_dir.join("system_prompt.md"), &skill.system_prompt)
        .map_err(|e| e.to_string())?;

    // Write slash_commands.json
    let cmds_json = serde_json::to_string_pretty(&skill.slash_commands)
        .unwrap_or_else(|_| "[]".to_string());
    std::fs::write(skill_dir.join("slash_commands.json"), &cmds_json)
        .map_err(|e| e.to_string())?;

    let manifest = SkillManifest {
        id: skill.id.clone(),
        name: skill.name.clone(),
        description: skill.description.clone(),
        version: skill.version.clone(),
        author: skill.author.clone(),
        tags: skill.tags.clone(),
        enabled: false,
        path: skill_dir.to_string_lossy().to_string(),
        source: "user".to_string(),
    };

    write_manifest(&manifest.path, &manifest)?;
    Ok(manifest)
}

/// Aggregate slash commands from all enabled skills.
#[tauri::command]
pub async fn list_slash_commands(app: AppHandle) -> Result<Vec<SlashCommand>, String> {
    let skills = list_skills(app.clone()).await?;
    let mut commands = Vec::new();
    for skill in skills.iter().filter(|s| s.enabled) {
        let path = PathBuf::from(&skill.path).join("slash_commands.json");
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(cmds) = serde_json::from_str::<Vec<SlashCommand>>(&raw) {
                commands.extend(cmds);
            }
        }
    }
    Ok(commands)
}
