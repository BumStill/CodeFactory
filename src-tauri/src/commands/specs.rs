// SPDX-License-Identifier: Apache-2.0
//! Spec Workbench commands — CRUD for `.codefactory/specs/*.md` files with
//! manual YAML frontmatter parsing (no extra crate).

use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::State;

use crate::AppState;

// ── Types ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecMeta {
    pub req_id: Option<String>,
    pub title: String,
    pub status: String, // draft | review | approved | implementing | done
    pub created_at: String,
    pub updated_at: String,
    pub tags: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub file_path: String, // absolute path to the .md file
    pub rel_path: String,  // relative to cwd
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecFile {
    pub meta: SpecMeta,
    pub content: String, // full markdown including frontmatter
    pub body: String,    // markdown body without frontmatter
}

// ── Frontmatter parsing ──────────────────────────────────────────────────────

/// Split `content` into (frontmatter_str, body_str).
/// Returns ("", content) when no frontmatter block is found.
fn split_frontmatter(content: &str) -> (&str, &str) {
    // Must start with "---\n" or "---\r\n"
    let rest = if let Some(r) = content.strip_prefix("---\n") {
        r
    } else if let Some(r) = content.strip_prefix("---\r\n") {
        r
    } else {
        return ("", content);
    };

    // Find the closing "---"
    if let Some(end) = rest.find("\n---\n").or_else(|| rest.find("\n---\r\n")) {
        let fm = &rest[..end];
        let body_start = end + if rest[end..].starts_with("\n---\r\n") { 6 } else { 5 };
        let body = rest.get(body_start..).unwrap_or("").trim_start_matches('\n');
        (fm, body)
    } else {
        ("", content)
    }
}

/// Parse a minimal YAML-ish frontmatter string into SpecMeta fields.
fn parse_frontmatter(fm: &str, file_path: &str, rel_path: &str) -> SpecMeta {
    let mut req_id: Option<String> = None;
    let mut title = String::new();
    let mut status = "draft".to_string();
    let mut created_at = Utc::now().to_rfc3339();
    let mut updated_at = Utc::now().to_rfc3339();
    let mut tags: Vec<String> = Vec::new();
    let mut acceptance_criteria: Vec<String> = Vec::new();

    // Simple state machine: track whether we're inside a list block
    #[derive(PartialEq)]
    enum Block {
        None,
        Tags,
        AcceptanceCriteria,
    }

    let mut current_block = Block::None;

    for line in fm.lines() {
        // List item: "  - value" or "- value"
        if let Some(item) = line.trim_start().strip_prefix("- ") {
            match current_block {
                Block::Tags => tags.push(item.trim().to_string()),
                Block::AcceptanceCriteria => {
                    acceptance_criteria.push(item.trim().to_string())
                }
                Block::None => {}
            }
            continue;
        }

        // key: value
        if let Some(colon) = line.find(':') {
            let key = line[..colon].trim();
            let val = line[colon + 1..].trim().to_string();

            // Reset block unless this is an indented line (handled above)
            current_block = Block::None;

            match key {
                "req_id" if !val.is_empty() => req_id = Some(val),
                "title" if !val.is_empty() => title = val,
                "status" if !val.is_empty() => status = val,
                "created_at" if !val.is_empty() => created_at = val,
                "updated_at" if !val.is_empty() => updated_at = val,
                "tags" => {
                    // Inline: tags: [a, b] or block
                    if val.starts_with('[') {
                        let inner = val.trim_matches(|c| c == '[' || c == ']');
                        tags = inner
                            .split(',')
                            .map(|t| t.trim().to_string())
                            .filter(|t| !t.is_empty())
                            .collect();
                    } else {
                        current_block = Block::Tags;
                    }
                }
                "acceptance_criteria" => {
                    current_block = Block::AcceptanceCriteria;
                }
                _ => {}
            }
        }
    }

    SpecMeta {
        req_id,
        title,
        status,
        created_at,
        updated_at,
        tags,
        acceptance_criteria,
        file_path: file_path.to_string(),
        rel_path: rel_path.to_string(),
    }
}

/// Update a single frontmatter key:value line within `content`.
/// If the key does not exist it is appended before the closing `---`.
fn update_frontmatter_key(content: &str, key: &str, value: &str) -> String {
    let (fm, body) = split_frontmatter(content);
    if fm.is_empty() {
        return content.to_string();
    }

    let mut lines: Vec<String> = fm.lines().map(str::to_string).collect();
    let prefix = format!("{key}:");
    let new_line = format!("{key}: {value}");

    let mut found = false;
    for line in lines.iter_mut() {
        if line.trim_start().starts_with(&prefix) {
            *line = new_line.clone();
            found = true;
            break;
        }
    }
    if !found {
        lines.push(new_line);
    }

    format!("---\n{}\n---\n{}", lines.join("\n"), body)
}

// ── Path helpers ─────────────────────────────────────────────────────────────

fn specs_dir(cwd: &str) -> PathBuf {
    Path::new(cwd).join(".codefactory").join("specs")
}

fn slugify(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn rel_path(base: &str, abs: &Path) -> String {
    abs.strip_prefix(base)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| abs.to_string_lossy().to_string())
}

// ── Spec template ────────────────────────────────────────────────────────────

fn spec_template(req_id: &str, title: &str, now: &str) -> String {
    format!(
        r#"---
req_id: {req_id}
title: {title}
status: draft
created_at: {now}
updated_at: {now}
tags: []
acceptance_criteria:
  - Replace with a concrete, testable acceptance criterion
---

# {title}

## Overview

Describe the feature or change at a high level.

## Requirements

| Req ID | Description | Validation |
|--------|-------------|------------|
| {req_id}-R1 | Describe the first requirement | How to verify it |

## Decision Points

<!-- DECISION: Should ... or ...? Pick the approach that best fits the project conventions. -->

## Testing Matrix

| Scenario | Expected Result | Pass/Fail |
|----------|-----------------|-----------|
| Happy path | ... | |
| Edge case | ... | |
"#
    )
}

// ── Commands ─────────────────────────────────────────────────────────────────

/// Scan `{cwd}/.codefactory/specs/**/*.md`, parse frontmatter, return list
/// sorted by `updated_at` descending.
#[tauri::command]
pub async fn list_specs(cwd: String) -> Result<Vec<SpecMeta>, String> {
    let dir = specs_dir(&cwd);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut metas: Vec<(String, SpecMeta)> = Vec::new();

    fn walk(dir: &Path, cwd: &str, metas: &mut Vec<(String, SpecMeta)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, cwd, metas);
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                let abs = path.to_string_lossy().to_string();
                let rel = rel_path(cwd, &path);
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let (fm, _) = split_frontmatter(&content);
                    let meta = parse_frontmatter(fm, &abs, &rel);
                    let sort_key = meta.updated_at.clone();
                    metas.push((sort_key, meta));
                }
            }
        }
    }

    walk(&dir, &cwd, &mut metas);
    metas.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(metas.into_iter().map(|(_, m)| m).collect())
}

/// Read a single spec file; parse frontmatter + body.
#[tauri::command]
pub async fn get_spec(path: String) -> Result<SpecFile, String> {
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("Cannot read spec: {e}"))?;
    let (fm_str, body_str) = {
        let (fm, body) = split_frontmatter(&content);
        (fm.to_string(), body.to_string())
    };
    let rel = path.clone(); // best-effort — caller can pass relative
    let meta = parse_frontmatter(&fm_str, &path, &rel);
    Ok(SpecFile {
        meta,
        content,
        body: body_str,
    })
}

/// Write `content` to `path`, create parent dirs as needed, return updated meta.
#[tauri::command]
pub async fn save_spec(path: String, content: String) -> Result<SpecMeta, String> {
    let p = Path::new(&path);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Cannot create dirs: {e}"))?;
    }
    std::fs::write(&path, &content).map_err(|e| format!("Cannot write spec: {e}"))?;
    let (fm, _) = split_frontmatter(&content);
    let meta = parse_frontmatter(fm, &path, &path);
    Ok(meta)
}

/// Create a new spec file from template in `{cwd}/.codefactory/specs/`.
#[tauri::command]
pub async fn create_spec(cwd: String, title: String) -> Result<SpecFile, String> {
    let dir = specs_dir(&cwd);

    // Count existing specs to generate next id.
    let count = if dir.exists() {
        std::fs::read_dir(&dir)
            .map(|e| e.flatten().filter(|x| x.path().extension().and_then(|ext| ext.to_str()) == Some("md")).count())
            .unwrap_or(0)
    } else {
        0
    };

    let req_id = format!("CF-{:03}", count + 1);
    let now = Utc::now().to_rfc3339();
    let slug = slugify(&title);
    let filename = format!("{}.md", slug);

    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create specs dir: {e}"))?;

    let file_path = dir.join(&filename);
    let content = spec_template(&req_id, &title, &now);

    std::fs::write(&file_path, &content).map_err(|e| format!("Cannot write spec: {e}"))?;

    let abs = file_path.to_string_lossy().to_string();
    let rel = rel_path(&cwd, &file_path);
    let (fm_str, body_str) = {
        let (fm, body) = split_frontmatter(&content);
        (fm.to_string(), body.to_string())
    };
    let meta = parse_frontmatter(&fm_str, &abs, &rel);
    Ok(SpecFile {
        meta,
        content,
        body: body_str,
    })
}

/// Delete the spec file at `path`.
#[tauri::command]
pub async fn delete_spec(path: String) -> Result<(), String> {
    std::fs::remove_file(&path).map_err(|e| format!("Cannot delete spec: {e}"))
}

/// Set status to "approved" and save.
#[tauri::command]
pub async fn approve_spec(path: String) -> Result<SpecMeta, String> {
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("Cannot read spec: {e}"))?;
    let now = Utc::now().to_rfc3339();
    let updated = update_frontmatter_key(&content, "status", "approved");
    let updated = update_frontmatter_key(&updated, "updated_at", &now);
    std::fs::write(&path, &updated).map_err(|e| format!("Cannot write spec: {e}"))?;
    let (fm, _) = split_frontmatter(&updated);
    let meta = parse_frontmatter(fm, &path, &path);
    Ok(meta)
}

// ── AI assist (non-streaming) ────────────────────────────────────────────────

#[derive(Serialize)]
struct AiMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct AiRequest {
    model: String,
    messages: Vec<AiMessage>,
    stream: bool,
    temperature: f32,
    max_tokens: u32,
}

#[derive(Deserialize)]
struct AiResponse {
    choices: Vec<AiChoice>,
}

#[derive(Deserialize)]
struct AiChoice {
    message: AiChoiceMessage,
}

#[derive(Deserialize)]
struct AiChoiceMessage {
    content: Option<String>,
}

/// Make a single non-streaming AI call with `instruction` + `spec_content`
/// and return the response text. Uses the configured default endpoint/model.
#[tauri::command]
pub async fn spec_ai_assist(
    spec_content: String,
    instruction: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let settings = state.settings.read().await.clone();
    let ep_name = &settings.default_endpoint;
    let model = settings.default_model.clone();

    let endpoint = settings
        .endpoints
        .get(ep_name)
        .ok_or_else(|| format!("Endpoint '{}' not configured", ep_name))?;

    let api_key = if let Some(ref key_ref) = endpoint.key_ref {
        crate::secrets::get_key(key_ref)
            .map_err(|e| format!("Failed to load API key: {e}"))?
            .unwrap_or_default()
    } else {
        String::new()
    };

    let base_url = endpoint.base_url.trim_end_matches('/');
    let url = format!("{base_url}/chat/completions");

    let system = "You are an expert software engineer and technical writer. \
                   You help write clear, structured software specification documents. \
                   Output only the requested content — no commentary, no markdown fences wrapping the whole output.";

    let user_content = if spec_content.is_empty() {
        instruction.clone()
    } else {
        format!(
            "{instruction}\n\n---\nCurrent spec content:\n{spec_content}"
        )
    };

    let request = AiRequest {
        model,
        messages: vec![
            AiMessage {
                role: "system".into(),
                content: system.into(),
            },
            AiMessage {
                role: "user".into(),
                content: user_content,
            },
        ],
        stream: false,
        temperature: 0.3,
        max_tokens: 2048,
    };

    let client = Client::new();
    let resp = client
        .post(&url)
        .bearer_auth(&api_key)
        .header("X-Title", "CodeFactory")
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("API error: {e}"))?
        .json::<AiResponse>()
        .await
        .map_err(|e| format!("JSON parse error: {e}"))?;

    let text = resp
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.message.content)
        .unwrap_or_default();

    Ok(text)
}

// ── AI task decomposition ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecomposedTask {
    pub tmp_id: String,
    pub title: String,
    pub description: String,
    pub dependencies: Vec<String>,
}

/// AI-powered task decomposition: takes spec content and returns a structured task list.
#[tauri::command]
pub async fn decompose_spec_to_tasks(
    spec_content: String,
    state: State<'_, AppState>,
) -> Result<Vec<DecomposedTask>, String> {
    let settings = state.settings.read().await.clone();
    let ep_name = &settings.default_endpoint;
    let model = settings.default_model.clone();

    let endpoint = settings
        .endpoints
        .get(ep_name)
        .ok_or_else(|| format!("Endpoint '{}' not configured", ep_name))?;

    let api_key = if let Some(ref key_ref) = endpoint.key_ref {
        crate::secrets::get_key(key_ref)
            .map_err(|e| format!("Failed to load API key: {e}"))?
            .unwrap_or_default()
    } else {
        String::new()
    };

    let base_url = endpoint.base_url.trim_end_matches('/');
    let url = format!("{base_url}/chat/completions");

    let prompt = format!(
        "You are a software project manager. Decompose this spec into a concrete list of implementation tasks for a development team.\n\n\
Return ONLY a JSON array (no markdown fences, no explanation), like:\n\
[\n  \
{{\"tmp_id\": \"t-0\", \"title\": \"...\", \"description\": \"...\", \"dependencies\": []}},\n  \
{{\"tmp_id\": \"t-1\", \"title\": \"...\", \"description\": \"...\", \"dependencies\": [\"t-0\"]}}\n\
]\n\n\
Rules:\n\
- 3-8 tasks maximum\n\
- Each task should be independently actionable\n\
- title: short (5-10 words), description: 1-2 sentences explaining what to implement\n\
- dependencies: list tmp_ids of tasks that must complete before this one (can be empty)\n\
- Focus on code changes, not process steps\n\n\
Spec:\n\
{spec_content}"
    );

    let request = AiRequest {
        model,
        messages: vec![AiMessage {
            role: "user".into(),
            content: prompt,
        }],
        stream: false,
        temperature: 0.3,
        max_tokens: 1024,
    };

    let client = Client::new();
    let resp = client
        .post(&url)
        .bearer_auth(&api_key)
        .header("X-Title", "CodeFactory")
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("API error: {e}"))?
        .json::<AiResponse>()
        .await
        .map_err(|e| format!("JSON parse error: {e}"))?;

    let text = resp
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.message.content)
        .unwrap_or_default();

    // Strip markdown fences if present
    let json_str = {
        let trimmed = text.trim();
        let stripped = trimmed
            .strip_prefix("```json")
            .or_else(|| trimmed.strip_prefix("```"))
            .map(|s| s.trim_end_matches("```").trim())
            .unwrap_or(trimmed);
        stripped.to_string()
    };

    let fallback = vec![DecomposedTask {
        tmp_id: "t-0".into(),
        title: "Implement spec".into(),
        description: "Implement the complete spec.".into(),
        dependencies: vec![],
    }];

    let parsed: Vec<serde_json::Value> = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(_) => return Ok(fallback),
    };

    if parsed.is_empty() {
        return Ok(fallback);
    }

    let tasks: Vec<DecomposedTask> = parsed
        .into_iter()
        .filter_map(|v| {
            let tmp_id = v["tmp_id"].as_str()?.to_string();
            let title = v["title"].as_str()?.to_string();
            let description = v["description"].as_str().unwrap_or("").to_string();
            let dependencies = v["dependencies"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|d| d.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            Some(DecomposedTask { tmp_id, title, description, dependencies })
        })
        .collect();

    if tasks.is_empty() {
        Ok(fallback)
    } else {
        Ok(tasks)
    }
}
