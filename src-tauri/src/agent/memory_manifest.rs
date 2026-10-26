// SPDX-License-Identifier: Apache-2.0
//! MEM-A 阶段 A：请求装配回执（影子模式，feature flag 默认关闭）。
//!
//! 现在这一轮请求到底发了哪几条、被丢弃了哪几条、哪几条是装配期合成的，事后
//! 只能靠复跑代码去猜。`build_openai_messages` 会丢掉被 completion gate 撤回的
//! 候选回答，也会为悬空工具调用合成 `recovered-tool-*` 结果，所以「线上消息」
//! 和「数据库行」本来就不是一一对应。
//!
//! 这里给每次装配留一张**只读回执**：来源 id / `seq` / `revision` / 角色 / 指纹，
//! 以及被排除项的原因。回执不参与装配，flag 关闭时连查都不查、写都不写。
//!
//! 红线（R3）：回执必须与请求出自同一次装配；写入失败只记日志，绝不能让一轮
//! 对话因为「记账」失败而中断。

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::{
    attachments, constraint_instruction_block, parse_tool_message_content,
    repair_incomplete_tool_history, replayable_history, replayed_command_repair_prompt,
};
use crate::openrouter::types::{ChatMessage, MessageContent, ToolCall};
use crate::storage::Message;

/// 灰度开关名。缺行 = 关闭，这是阶段 A 唯一的开关来源。
pub const FLAG_NAME: &str = "memory_assembly_manifest";
/// 无 GUI 入口的等价开关（headless / dispatch 直接设环境变量即可）。
pub const FLAG_ENV: &str = "CODEFACTORY_MEMORY_MANIFEST_SHADOW";
/// 装配期为悬空工具调用合成结果的 id 前缀（由 `repair_incomplete_tool_history` 产生）。
const SYNTHESIZED_TOOL_PREFIX: &str = "recovered-tool-";
/// 装配期追加的"上轮命令未收尾"提示，没有对应的历史行。
const INJECTED_COMMAND_REPAIR_ID: &str = "injected:command-repair";

/// 一条真正发出去的线上消息的来源。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestSource {
    /// 1-based 位置：相对于 wire 中 system 之后的第一条。
    pub ordinal: usize,
    /// 历史行 id；合成项是同前缀的合成 id，注入项是 `injected:*`。
    pub message_id: String,
    /// 历史行的排序身份（合成/注入项为 None）。
    pub seq: Option<i64>,
    /// 历史行的内容版本（合成/注入项为 0）。
    pub revision: i64,
    pub role: String,
    /// `history` | `synthesized` | `injected`
    pub origin: String,
    /// 该条线上消息的内容指纹，必须等于实际发出的那条。
    pub fingerprint: String,
}

/// 被排除、没有发出去的历史行，以及原因。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExcludedSource {
    pub message_id: String,
    pub reason: String,
}

/// 一次装配的完整回执（只读，不参与装配）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssemblyProvenance {
    pub sources: Vec<ManifestSource>,
    pub excluded: Vec<ExcludedSource>,
    /// 整条请求（system 之后部分）的聚合指纹。
    pub wire_fingerprint: String,
}

/// 稳定内容指纹（FNV-1a 64）。不引入新依赖，且跨进程/跨版本稳定，
/// 足以支撑"回执说发了这条、实际就发了这条"的逐项断言。
pub fn wire_fingerprint(message: &ChatMessage) -> String {
    let bytes = serde_json::to_vec(message).unwrap_or_default();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn source_origin(message: &Message) -> &'static str {
    if message.id.starts_with(SYNTHESIZED_TOOL_PREFIX) {
        "synthesized"
    } else {
        "history"
    }
}

/// 唯一的装配实现：返回线上消息**和**它们的来源。回执与请求出自同一次装配，
/// 所以"回执与实际发送逐项一致"是构造上成立的，而不是靠另跑一遍比对。
pub fn assemble_with_provenance(
    history: Vec<Message>,
    system_prompt: &str,
) -> (Vec<ChatMessage>, AssemblyProvenance) {
    // β (2026-09-28): a wording-level constraint reaches the model as an
    // INSTRUCTION here — the same place the turn's context is assembled — and
    // only when the turn actually carries one. It never touches capability.
    let turn_constraint = history
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .and_then(|message| constraint_instruction_block(&message.content));
    let system_text = match turn_constraint {
        Some(block) => format!("{system_prompt}\n\n{block}"),
        None => system_prompt.to_string(),
    };
    let input_identity: Vec<(String, Option<String>)> = history
        .iter()
        .map(|message| (message.id.clone(), message.completion_state.clone()))
        .collect();

    let mut msgs = vec![ChatMessage {
        role: "system".into(),
        content: MessageContent::Text(system_text),
        tool_calls: None,
        tool_call_id: None,
        name: None,
        reasoning_content: None,
    }];
    let repaired_history = repair_incomplete_tool_history(replayable_history(history));
    let replayed_command_repair = replayed_command_repair_prompt(&repaired_history);
    let mut sources: Vec<ManifestSource> = Vec::new();
    for m in repaired_history {
        let identity = (m.id.clone(), m.seq, m.revision, source_origin(&m));
        let message = match m.role.as_str() {
            "tool" => {
                // Content stored as: {"tool_call_id": "…", "content": "…"}
                let (tool_call_id, content) = parse_tool_message_content(&m.content);
                ChatMessage {
                    role: "tool".into(),
                    content: MessageContent::Text(content),
                    tool_calls: None,
                    tool_call_id: Some(tool_call_id),
                    name: None,
                    reasoning_content: None,
                }
            }
            "assistant" => {
                // Restore tool_calls if they were persisted as JSON
                let tool_calls: Option<Vec<ToolCall>> = m
                    .tool_calls
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok());
                ChatMessage {
                    role: "assistant".into(),
                    content: MessageContent::Text(m.content),
                    tool_calls,
                    tool_call_id: None,
                    name: None,
                    // Replay reasoning_content verbatim so DeepSeek reasoner
                    // models don't 400 on the next turn.
                    reasoning_content: m.reasoning_content,
                }
            }
            _ => {
                // Convert markdown file:// image links → vision content
                // parts when present, leaving plain text untouched.
                let parts = attachments::extract_openai_parts(&m.content);
                let content = if parts.is_empty() {
                    MessageContent::Text(m.content)
                } else {
                    MessageContent::Parts(parts)
                };
                ChatMessage {
                    role: m.role,
                    content,
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    reasoning_content: None,
                }
            }
        };
        let (message_id, seq, revision, origin) = identity;
        sources.push(ManifestSource {
            ordinal: sources.len() + 1,
            message_id,
            seq,
            revision,
            role: message.role.clone(),
            origin: origin.to_string(),
            fingerprint: wire_fingerprint(&message),
        });
        msgs.push(message);
    }
    if let Some(prompt) = replayed_command_repair {
        let message = ChatMessage {
            role: "user".into(),
            content: MessageContent::Text(prompt),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        };
        sources.push(ManifestSource {
            ordinal: sources.len() + 1,
            message_id: INJECTED_COMMAND_REPAIR_ID.into(),
            seq: None,
            revision: 0,
            role: message.role.clone(),
            origin: "injected".into(),
            fingerprint: wire_fingerprint(&message),
        });
        msgs.push(message);
    }

    let emitted: HashSet<&str> = sources
        .iter()
        .map(|source| source.message_id.as_str())
        .collect();
    let excluded = input_identity
        .into_iter()
        .filter(|(id, _)| !emitted.contains(id.as_str()))
        .map(|(message_id, state)| ExcludedSource {
            message_id,
            reason: match state {
                // Name the real mechanism, so the receipt is auditable
                // instead of saying "dropped".
                Some(state) => format!("completion_state={state} is not replayable"),
                None => "not replayable during assembly repair".to_string(),
            },
        })
        .collect();

    let wire_fingerprint = {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for source in &sources {
            for byte in source.fingerprint.as_bytes() {
                hash ^= *byte as u64;
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        format!("{hash:016x}")
    };

    (
        msgs,
        AssemblyProvenance {
            sources,
            excluded,
            wire_fingerprint,
        },
    )
}

/// 灰度开关：环境变量优先，其次 `feature_flags` 行；缺失/读失败一律视为关闭
/// （fail-closed：观察能力不能因为表缺失而变成新故障点）。
pub async fn flag_enabled(pool: &SqlitePool) -> bool {
    if let Ok(raw) = std::env::var(FLAG_ENV) {
        let value = raw.trim().to_ascii_lowercase();
        if matches!(value.as_str(), "1" | "true" | "on" | "yes") {
            return true;
        }
        if matches!(value.as_str(), "0" | "false" | "off" | "no") {
            return false;
        }
    }
    match sqlx::query_scalar::<_, i64>("SELECT enabled FROM feature_flags WHERE name = ?")
        .bind(FLAG_NAME)
        .fetch_optional(pool)
        .await
    {
        Ok(Some(enabled)) => enabled != 0,
        _ => false,
    }
}

/// 设置开关（headless / 测试 / 运维用；产品 UI 不暴露它）。
pub async fn set_flag(pool: &SqlitePool, enabled: bool) -> Result<(), sqlx::Error> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    sqlx::query(
        "INSERT INTO feature_flags (name, enabled, updated_at) VALUES (?, ?, ?) \
         ON CONFLICT(name) DO UPDATE SET enabled = excluded.enabled, updated_at = excluded.updated_at",
    )
    .bind(FLAG_NAME)
    .bind(i64::from(enabled))
    .bind(now)
    .execute(pool)
    .await
    .map(|_| ())
}

/// 把回执写进 `assembly_manifests`（影子模式唯一副作用）。
pub async fn manifest_record(
    pool: &SqlitePool,
    session_id: &str,
    provenance: &AssemblyProvenance,
) -> Result<(), sqlx::Error> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let payload = serde_json::to_string(provenance).unwrap_or_else(|_| "{}".to_string());
    sqlx::query(
        "INSERT INTO assembly_manifests \
         (session_id, created_at, flag_enabled, source_count, excluded_count, wire_fingerprint, payload_json) \
         VALUES (?, ?, 1, ?, ?, ?, ?)",
    )
    .bind(session_id)
    .bind(now)
    .bind(provenance.sources.len() as i64)
    .bind(provenance.excluded.len() as i64)
    .bind(&provenance.wire_fingerprint)
    .bind(payload)
    .execute(pool)
    .await
    .map(|_| ())
}

/// 生产装配入口：先装配，再（仅在开关打开时）留回执。
///
/// 顺序是刻意的——回执读取/写入的任何失败都只记一条 warn，绝不会让这一轮
/// 对话因为记账而失败（R3）。开关关闭时不查库、不写库，请求路径与之前完全一致。
pub async fn assemble_for_request(
    pool: &SqlitePool,
    session_id: &str,
    history: Vec<Message>,
    system_prompt: &str,
) -> Vec<ChatMessage> {
    let (wire, provenance) = assemble_with_provenance(history, system_prompt);
    if flag_enabled(pool).await {
        if let Err(error) = manifest_record(pool, session_id, &provenance).await {
            tracing::warn!(
                session_id = %session_id,
                error = %error,
                "memory assembly manifest could not be recorded (shadow mode; request unchanged)"
            );
        }
    }
    wire
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::storage::Message;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use sqlx::SqlitePool;

    async fn test_pool(name: &str) -> (tempfile::TempDir, SqlitePool) {
        let root = tempfile::tempdir().unwrap();
        let url = format!(
            "sqlite:{}",
            root.path().join(format!("{name}.db")).display()
        );
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                url.parse::<SqliteConnectOptions>()
                    .unwrap()
                    .create_if_missing(true)
                    .foreign_keys(false),
            )
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        (root, pool)
    }

    fn message(id: &str, role: &str, content: &str, seq: i64) -> Message {
        Message {
            id: id.into(),
            session_id: "s".into(),
            role: role.into(),
            content: content.into(),
            endpoint_id: None,
            model_id: None,
            input_tokens: None,
            output_tokens: None,
            tool_calls: None,
            reasoning_content: None,
            completion_state: None,
            created_at: 1_000 + seq,
            seq: Some(seq),
            revision: 1,
        }
    }

    /// 一段同时覆盖「正常回放」「被 gate 撤回的候选」「工具调用 + 结果」「悬空工具
    /// 调用需要装配期合成」的历史。
    fn representative_history() -> Vec<Message> {
        let mut user = message("u1", "user", "请读一下 src/main.rs", 1);

        let mut withdrawn = message("a-withdrawn", "assistant", "我做完了！", 2);
        withdrawn.completion_state = Some("rejected_candidate".into());

        let mut tool_call = message("a-tool", "assistant", "", 3);
        tool_call.tool_calls = Some(
            serde_json::json!([{
                "id": "call_1",
                "type": "function",
                "function": {"name": "read_file", "arguments": "{\"path\":\"src/main.rs\"}"}
            }])
            .to_string(),
        );

        // 刻意不给 call_1 结果：装配期必须合成一条 tool 结果。
        user.revision = 1;
        vec![user, withdrawn, tool_call]
    }

    /// R2 / R4：回执每一项都必须与真正发出去的那条消息对得上（角色 + 内容指纹），
    /// 被排除的候选带原因，合成项标注来源。
    #[tokio::test]
    async fn manifest_matches_sent_wire_item_by_item() {
        let (_root, pool) = test_pool("manifest-items").await;
        let history = representative_history();

        let (wire, provenance) = assemble_with_provenance(history.clone(), "SYSTEM");
        assert_eq!(wire[0].role, "system");
        assert!(
            wire.len() > 2,
            "工具调用悬空时必须合成结果，wire 至少 system+assistant+tool"
        );
        assert_eq!(
            provenance.sources.len(),
            wire.len() - 1,
            "回执必须逐项覆盖 system 之后的每一条线上消息"
        );

        for (index, source) in provenance.sources.iter().enumerate() {
            let sent = &wire[index + 1];
            assert_eq!(source.ordinal, index + 1, "序号必须与线上位置一致");
            assert_eq!(
                source.role,
                sent.role,
                "第 {} 项角色与线上消息不一致",
                index + 1
            );
            assert_eq!(
                source.fingerprint,
                wire_fingerprint(sent),
                "第 {} 项指纹必须等于真正发出的内容",
                index + 1
            );
        }

        assert!(
            provenance
                .sources
                .iter()
                .any(|s| s.message_id == "u1" && s.seq == Some(1) && s.revision == 1),
            "普通历史行必须带上 id/seq/revision"
        );

        let excluded = provenance
            .excluded
            .iter()
            .find(|e| e.message_id == "a-withdrawn")
            .expect("被 gate 撤回的候选必须出现在排除清单里");
        assert!(
            excluded.reason.contains("rejected_candidate"),
            "排除原因要能指向真实机制，实际: {}",
            excluded.reason
        );

        let synthesized = provenance
            .sources
            .iter()
            .find(|s| s.origin == "synthesized")
            .expect("悬空工具调用的合成结果必须标注为装配期合成");
        assert!(
            synthesized.message_id.starts_with("recovered-tool-"),
            "合成项应能指回合成来源，实际: {}",
            synthesized.message_id
        );

        // 写入回执后仍能读回，且与内存里的回执一致。
        manifest_record(&pool, "s", &provenance).await.unwrap();
        let payload: String = sqlx::query_scalar(
        "SELECT payload_json FROM assembly_manifests WHERE session_id='s' ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
        let stored: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(
            stored["sources"].as_array().map(|a| a.len()),
            Some(provenance.sources.len()),
            "落库回执的条目数必须与内存回执一致"
        );
        pool.close().await;
    }

    /// R2 / R3 / R4：flag 关与开，请求字节完全相同——回执只是观察，不是投影。
    #[tokio::test]
    async fn shadow_flag_off_and_on_produce_identical_request_bytes() {
        let (_root, pool) = test_pool("manifest-bytes").await;

        let off = assemble_for_request(&pool, "s", representative_history(), "SYSTEM").await;
        let bytes_off = serde_json::to_vec(&off).unwrap();
        let rows_off: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM assembly_manifests")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows_off, 0, "flag 默认关闭时不得写任何回执");

        set_flag(&pool, true).await.unwrap();
        let on = assemble_for_request(&pool, "s", representative_history(), "SYSTEM").await;
        let bytes_on = serde_json::to_vec(&on).unwrap();
        let rows_on: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM assembly_manifests")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows_on, 1, "flag 打开后每次装配应留一条回执");
        assert_eq!(
            bytes_off, bytes_on,
            "开启影子回执不得改变请求字节（同样的历史必须组装出同样的请求）"
        );
        pool.close().await;
    }

    /// R2：flag 缺省为关闭；缺失行、0 都算关闭。
    #[tokio::test]
    async fn flag_defaults_off_when_absent() {
        let (_root, pool) = test_pool("manifest-flag").await;
        assert!(!flag_enabled(&pool).await, "没有配置行时必须视为关闭");
        set_flag(&pool, false).await.unwrap();
        assert!(!flag_enabled(&pool).await, "显式 0 也是关闭");
        set_flag(&pool, true).await.unwrap();
        assert!(flag_enabled(&pool).await, "显式 1 才开启");
        pool.close().await;
    }

    /// R3：记账失败不能打断一轮对话——装配结果照常返回。
    #[tokio::test]
    async fn manifest_write_failure_does_not_break_assembly() {
        let (_root, pool) = test_pool("manifest-failure").await;
        set_flag(&pool, true).await.unwrap();
        sqlx::query("DROP TABLE assembly_manifests")
            .execute(&pool)
            .await
            .unwrap();

        let wire = assemble_for_request(&pool, "s", representative_history(), "SYSTEM").await;
        assert_eq!(wire[0].role, "system");
        assert!(wire.len() > 2, "回执写入失败时仍必须返回完整的装配结果");
        pool.close().await;
    }
}
