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
    install_user_skill_from_url(&url, false).await
}

/// Fetch a JSON skill manifest from `url` and write it to the user skills dir.
/// `enabled` controls whether it activates immediately. App-independent.
pub async fn install_user_skill_from_url(url: &str, enabled: bool) -> Result<SkillManifest, String> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("拉取失败: {e}"))?;
    let raw = response
        .text()
        .await
        .map_err(|e| format!("读取响应失败: {e}"))?;

    let mf: ManifestFile =
        serde_json::from_str(&raw).map_err(|e| format!("manifest JSON 无效: {e}"))?;

    let skill_dir = user_skills_dir().join(&mf.id);
    std::fs::create_dir_all(&skill_dir).map_err(|e| e.to_string())?;
    std::fs::write(
        skill_dir.join("system_prompt.md"),
        mf.system_prompt.clone().unwrap_or_default(),
    )
    .map_err(|e| e.to_string())?;

    let manifest = SkillManifest {
        id: mf.id,
        name: mf.name,
        description: mf.description,
        version: mf.version,
        author: mf.author,
        tags: mf.tags,
        enabled,
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
fn import_one_skill_dir(src: &Path, enabled: bool) -> Result<SkillManifest, String> {
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
        enabled,
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
        match import_one_skill_dir(&d, true) {
            Ok(m) => imported.push(m),
            Err(e) => tracing::warn!("skill 导入跳过 {}: {e}", d.display()),
        }
    }
    if imported.is_empty() {
        return Err("找到了 skill 目录但全部导入失败".to_string());
    }
    Ok(imported)
}

// ── Reusable, app-independent skill ops (shared by agent tools) ───────────────

/// Create a new USER skill on disk. Only touches the user skills dir (no
/// AppHandle needed), so the agent's skill tools can call it. Errors if the id
/// already exists — use [`update_user_skill`] to change one.
pub fn create_user_skill(
    name: &str,
    description: &str,
    instructions: &str,
    enabled: bool,
) -> Result<SkillManifest, String> {
    let id = slugify(name);
    let dest = user_skills_dir().join(&id);
    if dest.exists() {
        return Err(format!("技能 '{id}' 已存在，请改用更新（skill_update）"));
    }
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    let manifest = SkillManifest {
        id: id.clone(),
        name: name.to_string(),
        description: description.to_string(),
        version: "1.0.0".to_string(),
        author: "you".to_string(),
        tags: vec![],
        enabled,
        path: dest.to_string_lossy().to_string(),
        source: "user".to_string(),
    };
    write_manifest(&manifest.path, &manifest)?;
    std::fs::write(dest.join("system_prompt.md"), instructions).map_err(|e| e.to_string())?;
    Ok(manifest)
}

/// Update an existing USER skill's fields / instructions. Each `Some` is applied;
/// `None` leaves that field as-is. The enabled state is preserved.
pub fn update_user_skill(
    id: &str,
    name: Option<&str>,
    description: Option<&str>,
    instructions: Option<&str>,
) -> Result<SkillManifest, String> {
    let dir = user_skills_dir().join(id);
    if !dir.exists() {
        return Err(format!("用户技能 '{id}' 不存在"));
    }
    let raw = std::fs::read_to_string(dir.join("manifest.json")).map_err(|e| e.to_string())?;
    let mf: ManifestFile =
        serde_json::from_str(&raw).map_err(|e| format!("manifest.json 解析失败: {e}"))?;
    let manifest = SkillManifest {
        id: id.to_string(),
        name: name.map(String::from).unwrap_or(mf.name),
        description: description.map(String::from).unwrap_or(mf.description),
        version: mf.version,
        author: mf.author,
        tags: mf.tags,
        enabled: mf.enabled,
        path: dir.to_string_lossy().to_string(),
        source: "user".to_string(),
    };
    write_manifest(&manifest.path, &manifest)?;
    if let Some(instr) = instructions {
        std::fs::write(dir.join("system_prompt.md"), instr).map_err(|e| e.to_string())?;
    }
    Ok(manifest)
}

/// List the USER skills (those under the user skills dir). App-independent.
pub fn list_user_skills() -> Vec<SkillManifest> {
    scan_skill_dir(&user_skills_dir(), "user")
}

/// Delete a USER skill by id. App-independent.
pub fn delete_user_skill(id: &str) -> Result<(), String> {
    let dir = user_skills_dir().join(id);
    if !dir.exists() {
        return Err(format!("用户技能 '{id}' 不存在"));
    }
    std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())
}

/// Create a skill from the UI form. User-authored → enabled immediately.
#[tauri::command]
pub async fn create_skill(
    name: String,
    description: String,
    instructions: String,
) -> Result<SkillManifest, String> {
    create_user_skill(&name, &description, &instructions, true)
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

    std::fs::remove_dir_all(&manifest.path).map_err(|e| e.to_string())
}

/// The trimmed `system_prompt.md` body of every enabled skill, in list order.
/// The agent loop wraps each into a budgeted context block (see
/// `agent::context_budget`) rather than concatenating them unbounded.
pub async fn enabled_skill_prompts(app: &AppHandle) -> Vec<String> {
    let skills = match list_skills(app.clone()).await {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    skills
        .iter()
        .filter(|s| s.enabled)
        .filter_map(|s| {
            let path = PathBuf::from(&s.path).join("system_prompt.md");
            std::fs::read_to_string(path).ok()
        })
        .map(|p| p.trim().to_string())
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

// ── Agent-facing discovery / fetch (search the registry, install from a source) ─

/// The skill registry CodeFactory searches (same catalog as the Skills page).
/// Remote; falls back to the embedded BUILTIN_REGISTRY on any error.
const SKILL_REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/BumStill/codefactory-skills/main/registry.json";

/// Search the registry for installable skills matching `query` (name /
/// description / id / tags; empty query returns all). App-independent.
pub async fn search_registry_skills(query: &str) -> Vec<MarketplaceSkill> {
    let raw = match reqwest::get(SKILL_REGISTRY_URL).await {
        Ok(r) => r.text().await.unwrap_or_else(|_| BUILTIN_REGISTRY.to_string()),
        Err(_) => BUILTIN_REGISTRY.to_string(),
    };
    let all: Vec<MarketplaceSkill> = serde_json::from_str(&raw)
        .or_else(|_| serde_json::from_str(BUILTIN_REGISTRY))
        .unwrap_or_default();
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
    let skill = search_registry_skills("")
        .await
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| format!("registry 里没有 id 为 '{id}' 的技能（先用 skill_search 搜索）"))?;
    install_marketplace_skill(skill).await
}

/// Fetch + install skill(s) from a source, landing them DISABLED (the user
/// reviews + enables). Accepts, in order: an existing local directory, a git
/// repo URL (github/gitlab/*.git, shallow-cloned), a manifest JSON URL, or a
/// registry id. App-independent — backs the agent's `skill_fetch` tool.
pub async fn fetch_skill_from_source(source: &str) -> Result<Vec<SkillManifest>, String> {
    let s = source.trim();

    // 1) Existing local directory.
    let as_path = PathBuf::from(s);
    if as_path.is_dir() {
        let mut dirs = Vec::new();
        collect_skill_dirs(&as_path, 3, &mut dirs);
        let out: Vec<_> = dirs
            .iter()
            .filter_map(|d| import_one_skill_dir(d, false).ok())
            .collect();
        return if out.is_empty() {
            Err("目录里没找到可导入的 skill".into())
        } else {
            Ok(out)
        };
    }

    // 2) Git repo → shallow clone to a temp dir, import, clean up.
    if s.starts_with("http")
        && (s.contains("github.com") || s.contains("gitlab.com") || s.ends_with(".git"))
    {
        let tmp = std::env::temp_dir().join(format!("cf-skill-{}", slugify(s)));
        let _ = std::fs::remove_dir_all(&tmp);
        let url = s.to_string();
        let tmp_for_clone = tmp.clone();
        let status = tokio::task::spawn_blocking(move || {
            std::process::Command::new("git")
                .args(["clone", "--depth", "1", &url])
                .arg(&tmp_for_clone)
                .status()
        })
        .await
        .map_err(|e| format!("clone 任务失败: {e}"))?
        .map_err(|e| format!("git 不可用: {e}"))?;
        if !status.success() {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err("git clone 失败（检查仓库地址是否正确、可访问）".into());
        }
        let mut dirs = Vec::new();
        collect_skill_dirs(&tmp, 3, &mut dirs);
        let out: Vec<_> = dirs
            .iter()
            .filter_map(|d| import_one_skill_dir(d, false).ok())
            .collect();
        let _ = std::fs::remove_dir_all(&tmp);
        return if out.is_empty() {
            Err("仓库里没找到 skill（需要 SKILL.md 或 manifest.json）".into())
        } else {
            Ok(out)
        };
    }

    // 3) Plain http(s) URL → a JSON manifest.
    if s.starts_with("http") {
        return install_user_skill_from_url(s, false).await.map(|m| vec![m]);
    }

    // 4) Otherwise: treat as a registry id.
    install_marketplace_skill_by_id(s).await.map(|m| vec![m])
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

#[cfg(test)]
mod tests {
    use super::*;

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
            assert!(!s.system_prompt.trim().is_empty(), "{} has an empty system_prompt", s.id);
            assert!(!s.name.trim().is_empty(), "{} has an empty name", s.id);
            assert!(!s.slash_commands.is_empty(), "{} has no slash commands", s.id);
        }
    }
}
