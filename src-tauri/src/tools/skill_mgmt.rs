// SPDX-License-Identifier: Apache-2.0
//! Agent-facing skill management — lets the model create, update, list and
//! delete the user's skills mid-conversation ("做个写周报的技能", "把它改正式点").
//!
//! Created/updated skills land **disabled**: a skill is injected into the
//! system prompt only once enabled, and enabling is the user's call (the Skills
//! page). Read operations use the normal read policy; create/update/delete/fetch
//! use the normal mutation permission gate.

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
    match crate::commands::skills::create_user_skill(&a.name, &a.description, &a.instructions) {
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
                "已启用。"
            } else {
                "当前未启用；请重新检查内容后启用。"
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
            "registry 里没找到和「{}」匹配的技能。你也可以提供一个公开 HTTPS manifest URL；本机目录需要在资源中心用原生目录选择器导入。",
            a.query
        )));
    }
    let mut out = String::from("找到这些可安装技能(用 skill_fetch <id> 安装):\n");
    for s in results.iter().take(15) {
        out.push_str(&format!(
            "- {} (id: {}) — {}\n",
            s.name, s.id, s.description
        ));
    }
    Ok(ToolOutput::ok(out))
}

pub fn fetch_definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "skill_fetch".into(),
            description: "Install a skill from a source: a registry id (from skill_search) or a public \
                HTTPS manifest JSON URL. Raw local paths and Git sources are unavailable during security \
                containment; local directories must use the Resource Center picker. Installs DISABLED — \
                the tool result includes an exact review action; tell the user to use that action before enabling."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "source": { "type": "string", "description": "embedded registry id or public HTTPS manifest URL" }
                },
                "required": ["source"]
            }),
        },
    }
}

const SKILL_RECEIPT_PREFIX: &str = "CODEFACTORY_SKILL_RECEIPT_V1:";

fn skill_install_receipt(manifests: &[crate::commands::skills::SkillManifest]) -> Value {
    json!({
        "schema_version": 1,
        "kind": "skill_install",
        "items": manifests.iter().map(|manifest| json!({
            "id": manifest.id,
            "name": manifest.name,
            "version": manifest.version,
            "installed": true,
            "activation": "disabled",
        })).collect::<Vec<_>>(),
    })
}

fn skill_review_metadata(receipt: &Value) -> Value {
    json!({ "codefactory_ui": receipt })
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
            let receipt = skill_install_receipt(&ms);
            Ok(ToolOutput::ok(format!(
                "已获取 {} 个技能(均未启用):{names}。请使用本结果的检查入口预览实际内容。当前 root turn 的 Skill 快照不会变化；显式启用成功后，只有之后启动的新 root turn 才可加载。\n{SKILL_RECEIPT_PREFIX}{receipt}",
                ms.len(),
            ))
            .with_metadata(skill_review_metadata(&receipt)))
        }
        Err(e) => Ok(ToolOutput::err(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::{skill_install_receipt, skill_review_metadata, SKILL_RECEIPT_PREFIX};
    use crate::commands::skills::SkillManifest;

    #[test]
    fn skill_fetch_receipt_targets_exact_installed_ids_for_review() {
        let manifests = [SkillManifest {
            id: "continuity-helper".into(),
            name: "Continuity Helper".into(),
            description: "fixture".into(),
            version: "1.0.0".into(),
            author: "fixture".into(),
            tags: Vec::new(),
            enabled: false,
            path: "/fixture/continuity-helper".into(),
            source: "user".into(),
            lifecycle_status: "ready".into(),
        }];
        let receipt = skill_install_receipt(&manifests);
        let metadata = skill_review_metadata(&receipt);
        assert_eq!(metadata["codefactory_ui"]["schema_version"], 1);
        assert_eq!(metadata["codefactory_ui"]["kind"], "skill_install");
        assert_eq!(
            metadata["codefactory_ui"]["items"][0],
            serde_json::json!({
                "id": "continuity-helper",
                "name": "Continuity Helper",
                "version": "1.0.0",
                "installed": true,
                "activation": "disabled",
            })
        );
        assert!(format!("{SKILL_RECEIPT_PREFIX}{receipt}")
            .starts_with("CODEFACTORY_SKILL_RECEIPT_V1:{"));
    }
}
