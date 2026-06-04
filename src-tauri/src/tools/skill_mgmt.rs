// SPDX-License-Identifier: Apache-2.0
//! Agent-facing skill management — lets the model create, update, list and
//! delete the user's skills mid-conversation ("做个写周报的技能", "把它改正式点").
//!
//! Created/updated skills land **disabled**: a skill is injected into the
//! system prompt only once enabled, and enabling is the user's call (the Skills
//! page) — so the agent can author capabilities without silently rewriting its
//! own instructions. These tools therefore skip the permission prompt (see
//! `decide_permission`); the gate is the enable step.

use serde::Deserialize;
use serde_json::{json, Value};

use super::{ExecCtx, ToolOutput};
use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};

pub fn create_definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "skill_create".into(),
            description: "Create a new reusable skill for the user — a named capability whose \
                instructions get injected into your system prompt once enabled. The skill is \
                created DISABLED; tell the user to review and enable it on the Skills page. Use \
                this when the user asks you to make/build/author a skill."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Short human name, e.g. 周报助手" },
                    "description": { "type": "string", "description": "One line on when to use it" },
                    "instructions": { "type": "string", "description": "Full skill instructions (markdown), in the user's language" }
                },
                "required": ["name", "description", "instructions"]
            }),
        },
    }
}

pub async fn execute_create(args: Value, _ctx: &ExecCtx) -> Result<ToolOutput> {
    #[derive(Deserialize)]
    struct A {
        name: String,
        description: String,
        instructions: String,
    }
    let a: A = match serde_json::from_value(args) {
        Ok(v) => v,
        Err(e) => return Ok(ToolOutput::err(format!("skill_create 参数错误: {e}"))),
    };
    match crate::commands::skills::create_user_skill(&a.name, &a.description, &a.instructions, false)
    {
        Ok(m) => Ok(ToolOutput::ok(format!(
            "已创建技能「{}」(id: {})，当前未启用。请到 Skills 页预览内容并启用后即生效。",
            m.name, m.id
        ))),
        Err(e) => Ok(ToolOutput::err(e)),
    }
}

pub fn update_definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "skill_update".into(),
            description: "Update an existing USER skill: change its name, description, and/or \
                instructions. Only the provided fields change. Call skill_list first for the id."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "The skill id (from skill_list)" },
                    "name": { "type": "string" },
                    "description": { "type": "string" },
                    "instructions": { "type": "string", "description": "Replacement instructions (markdown)" }
                },
                "required": ["id"]
            }),
        },
    }
}

pub async fn execute_update(args: Value, _ctx: &ExecCtx) -> Result<ToolOutput> {
    #[derive(Deserialize)]
    struct A {
        id: String,
        name: Option<String>,
        description: Option<String>,
        instructions: Option<String>,
    }
    let a: A = match serde_json::from_value(args) {
        Ok(v) => v,
        Err(e) => return Ok(ToolOutput::err(format!("skill_update 参数错误: {e}"))),
    };
    match crate::commands::skills::update_user_skill(
        &a.id,
        a.name.as_deref(),
        a.description.as_deref(),
        a.instructions.as_deref(),
    ) {
        Ok(m) => Ok(ToolOutput::ok(format!(
            "已更新技能「{}」(id: {})。{}",
            m.name,
            m.id,
            if m.enabled {
                "已启用，改动即时生效。"
            } else {
                "当前未启用。"
            }
        ))),
        Err(e) => Ok(ToolOutput::err(e)),
    }
}

pub fn list_definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "skill_list".into(),
            description: "List the user's own skills (id, name, enabled state, description). Use \
                this before updating or deleting a skill."
                .into(),
            parameters: json!({ "type": "object", "properties": {} }),
        },
    }
}

pub async fn execute_list(_args: Value, _ctx: &ExecCtx) -> Result<ToolOutput> {
    let skills = crate::commands::skills::list_user_skills();
    if skills.is_empty() {
        return Ok(ToolOutput::ok("用户还没有自建技能。".to_string()));
    }
    let mut out = String::from("用户技能：\n");
    for s in skills {
        out.push_str(&format!(
            "- {} (id: {}) [{}] — {}\n",
            s.name,
            s.id,
            if s.enabled { "已启用" } else { "未启用" },
            s.description
        ));
    }
    Ok(ToolOutput::ok(out))
}

pub fn delete_definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "skill_delete".into(),
            description: "Delete a USER skill by id. Built-in skills cannot be deleted.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }),
        },
    }
}

pub async fn execute_delete(args: Value, _ctx: &ExecCtx) -> Result<ToolOutput> {
    #[derive(Deserialize)]
    struct A {
        id: String,
    }
    let a: A = match serde_json::from_value(args) {
        Ok(v) => v,
        Err(e) => return Ok(ToolOutput::err(format!("skill_delete 参数错误: {e}"))),
    };
    match crate::commands::skills::delete_user_skill(&a.id) {
        Ok(()) => Ok(ToolOutput::ok(format!("已删除技能 {}。", a.id))),
        Err(e) => Ok(ToolOutput::err(e)),
    }
}

pub fn search_definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "skill_search".into(),
            description: "Search the skill registry for installable skills matching a query \
                (by name / description / tags). Use this when the user wants you to find a skill \
                for some capability. Then install one with skill_fetch <id>."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What capability to look for, e.g. PDF 表格 / code review" }
                },
                "required": ["query"]
            }),
        },
    }
}

pub async fn execute_search(args: Value, _ctx: &ExecCtx) -> Result<ToolOutput> {
    #[derive(Deserialize)]
    struct A {
        query: String,
    }
    let a: A = match serde_json::from_value(args) {
        Ok(v) => v,
        Err(e) => return Ok(ToolOutput::err(format!("skill_search 参数错误: {e}"))),
    };
    let results = crate::commands::skills::search_registry_skills(&a.query).await;
    if results.is_empty() {
        return Ok(ToolOutput::ok(format!(
            "registry 里没找到和「{}」匹配的技能。你也可以给我一个 GitHub 仓库或 SKILL.md 链接,用 skill_fetch 直接装。",
            a.query
        )));
    }
    let mut out = String::from("找到这些可安装技能(用 skill_fetch <id> 安装):\n");
    for s in results.iter().take(15) {
        out.push_str(&format!("- {} (id: {}) — {}\n", s.name, s.id, s.description));
    }
    Ok(ToolOutput::ok(out))
}

pub fn fetch_definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "skill_fetch".into(),
            description: "Install a skill from a source: a registry id (from skill_search), a git \
                repo URL (github/gitlab — shallow-cloned, finds every SKILL.md), a manifest JSON \
                URL, or a local directory path. Installs DISABLED — tell the user to review and \
                enable it on the Skills page."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "source": { "type": "string", "description": "registry id, git URL, JSON URL, or a local directory path" }
                },
                "required": ["source"]
            }),
        },
    }
}

pub async fn execute_fetch(args: Value, _ctx: &ExecCtx) -> Result<ToolOutput> {
    #[derive(Deserialize)]
    struct A {
        source: String,
    }
    let a: A = match serde_json::from_value(args) {
        Ok(v) => v,
        Err(e) => return Ok(ToolOutput::err(format!("skill_fetch 参数错误: {e}"))),
    };
    match crate::commands::skills::fetch_skill_from_source(&a.source).await {
        Ok(ms) => {
            let names = ms
                .iter()
                .map(|m| format!("「{}」(id: {})", m.name, m.id))
                .collect::<Vec<_>>()
                .join("、");
            Ok(ToolOutput::ok(format!(
                "已获取 {} 个技能(均未启用):{names}。请到 Skills 页预览内容并启用后即生效。",
                ms.len()
            )))
        }
        Err(e) => Ok(ToolOutput::err(e)),
    }
}
