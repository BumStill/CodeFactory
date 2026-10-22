// SPDX-License-Identifier: Apache-2.0
//! 阶段 2 / M154：把「回合中途被杀」留下的历史修成协议合法的形状。
//!
//! 一次回合在工具执行中途中断（进程被杀、窗口关闭、强制重启）会在会话里留下
//! 两种残缺状态，任何一种都会让下一次请求被 provider 直接拒绝（HTTP 400，几秒
//! 内失败、且不会落库任何 assistant 消息）：
//!
//! 1. **悬空 tool call**：assistant 消息带着 `tool_calls=[call_1]` 落了库，但
//!    `call_1` 的 tool 结果从未落库（工具还在跑就被杀了）。OpenAI Responses 的
//!    `function_call` 后面必须有 `function_call_output`，否则报
//!    "No tool output found for function call"。
//! 2. **孤立 tool 结果**：tool 结果落了库，但发起它的 assistant 消息没落库。
//!    所有兼容 OpenAI 的 provider 都会拒绝没有对应调用 id 的 tool 消息。
//!
//! 3. 断点续跑的提示语过去是以 `system` 角色**追加**在历史末尾的。ChatGPT /
//!    Anthropic 两条 wire 路径对 `system` 都是**赋值而不是追加**
//!    （`model_transport.rs` 里 `instructions = ...` / `system = ...`），于是末尾
//!    那条短提示会把整段系统提示词覆盖掉；而部分 provider 直接拒绝出现在对话
//!    中间或末尾的 system 消息。
//!
//! 这里在唯一的 loop 入口做一次幂等修复：保留模型原本的调用意图（悬空调用补一条
//! 明确的「结果未知」tool 结果），丢掉无法解释的孤立结果，并把非首条 system 消息
//! 降级成 user 消息。干净的往返历史原样通过。

use crate::types::{ChatMessage, MessageContent};

/// 悬空工具调用的占位结果。必须明确写「结果未知」，不能让模型以为它成功或失败。
pub const INTERRUPTED_TOOL_RESULT: &str =
    "（工具未返回结果：上一回合在工具执行期间被中断，该工具可能已执行也可能未执行，结果未知）";

/// 幂等修复历史。干净的对话（每个 tool call 都有结果、system 只在开头）原样返回。
pub fn repair_interrupted_history(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::with_capacity(messages.len() + 4);
    // 已发出、还没等到结果就开始的 tool call id，按顺序排队。
    let mut open: Vec<String> = Vec::new();

    for mut message in messages {
        // 只有历史第一条可以是 system。续跑提示、压缩标记等后来追加的 system
        // 消息一律降级为 user：语义仍然是「给模型的一段话」，但不会覆盖系统
        // 提示词，也不会被拒绝「system 不在开头」的 provider 打回。
        if message.role == "system" && !out.is_empty() {
            message.role = "user".into();
        }

        match message.role.as_str() {
            "assistant" => {
                // 新的一轮 assistant 开始时，上一轮还没结果的调用已经不可能再
                // 有结果了，先补齐，再接新消息。
                close_open_calls(&mut out, &mut open);
                if let Some(calls) = message.tool_calls.as_ref() {
                    open.extend(calls.iter().map(|call| call.id.clone()));
                }
                out.push(message);
            }
            "tool" => {
                let Some(call_id) = message.tool_call_id.clone() else {
                    // 没有调用 id 的 tool 消息无法对应任何调用，provider 会拒绝。
                    continue;
                };
                match open.iter().position(|open_id| *open_id == call_id) {
                    Some(position) => {
                        open.remove(position);
                        out.push(message);
                    }
                    // 孤立结果：发起它的 assistant 消息没落库。丢掉它，模型从
                    // 「结果未知」这件事本身已经无法再获得信息。
                    None => continue,
                }
            }
            _ => {
                // user 等非 tool 消息出现时，未闭合的调用不会再有结果。
                close_open_calls(&mut out, &mut open);
                out.push(message);
            }
        }
    }

    close_open_calls(&mut out, &mut open);
    out
}

fn close_open_calls(out: &mut Vec<ChatMessage>, open: &mut Vec<String>) {
    for call_id in open.drain(..) {
        out.push(ChatMessage {
            role: "tool".into(),
            content: MessageContent::Text(INTERRUPTED_TOOL_RESULT.into()),
            tool_calls: None,
            tool_call_id: Some(call_id),
            name: None,
            reasoning_content: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FunctionCall, ToolCall};

    fn text(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: MessageContent::Text(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        }
    }

    fn assistant_call(call_id: &str) -> ChatMessage {
        ChatMessage {
            role: "assistant".into(),
            content: MessageContent::Text(String::new()),
            tool_calls: Some(vec![ToolCall {
                id: call_id.into(),
                r#type: "function".into(),
                function: FunctionCall {
                    name: "bash".into(),
                    arguments: "{\"command\":\"sleep 60\"}".into(),
                },
            }]),
            tool_call_id: None,
            name: None,
            reasoning_content: None,
        }
    }

    fn tool_result(call_id: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: "tool".into(),
            content: MessageContent::Text(content.into()),
            tool_calls: None,
            tool_call_id: Some(call_id.into()),
            name: None,
            reasoning_content: None,
        }
    }

    /// 干净的往返历史必须逐字原样通过（幂等 + 不误伤正常对话）。
    #[test]
    fn clean_history_passes_through_unchanged() {
        let history = vec![
            text("system", "you are codefactory"),
            text("user", "run the tests"),
            assistant_call("call_1"),
            tool_result("call_1", "ok"),
            text("assistant", "done"),
        ];
        let repaired = repair_interrupted_history(history.clone());
        assert_eq!(repaired.len(), history.len());
        for (before, after) in history.iter().zip(repaired.iter()) {
            assert_eq!(before.role, after.role);
            assert_eq!(before.tool_call_id, after.tool_call_id);
            assert_eq!(before.tool_calls.is_some(), after.tool_calls.is_some());
        }
        // 幂等：再修一次不变。
        assert_eq!(repair_interrupted_history(repaired.clone()).len(), repaired.len());
    }

    /// 断点续跑过去的形状：末尾追加一条 system 提示。修复后最后一条必须是 user。
    #[test]
    fn trailing_system_resume_hint_becomes_user() {
        let history = vec![
            text("system", "you are codefactory"),
            text("user", "只读演练：sleep 60"),
            text("system", "你在 … 被中断，先用 git status 确认现状"),
        ];
        let repaired = repair_interrupted_history(history);
        assert_eq!(repaired[0].role, "system");
        assert_eq!(repaired.last().unwrap().role, "user");
        assert_eq!(repaired.len(), 3);
    }

    /// 悬空 tool call（工具跑着被杀，结果从未落库）必须补一条明确的「结果未知」。
    #[test]
    fn dangling_tool_call_gets_unknown_result() {
        let repaired = repair_interrupted_history(vec![
            text("system", "you are codefactory"),
            text("user", "只读演练"),
            assistant_call("call_1"),
        ]);
        assert_eq!(repaired.last().unwrap().role, "tool");
        assert_eq!(
            repaired.last().unwrap().tool_call_id.as_deref(),
            Some("call_1")
        );
        match &repaired.last().unwrap().content {
            MessageContent::Text(value) => assert!(value.contains("结果未知"), "{value}"),
            other => panic!("unexpected content: {other:?}"),
        }
        assert_eq!(repaired.len(), 4);
    }

    /// 悬空调用后面还跟着一条新 user 消息（用户点了「继续」）时，补齐必须
    /// 插在 user 之前，保持 tool → user 的顺序。
    #[test]
    fn dangling_tool_call_is_closed_before_the_next_user_turn() {
        let repaired = repair_interrupted_history(vec![
            text("user", "只读演练"),
            assistant_call("call_1"),
            text("user", "继续"),
        ]);
        assert_eq!(repaired.len(), 4);
        assert_eq!(repaired[2].role, "tool");
        assert_eq!(repaired[2].tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(repaired[3].role, "user");
    }

    /// 孤立 tool 结果（发起它的 assistant 消息没落库）必须被丢掉。
    #[test]
    fn orphan_tool_result_is_dropped() {
        let repaired = repair_interrupted_history(vec![
            text("user", "只读演练"),
            tool_result("call_ghost", "LIVENESS-DONE"),
        ]);
        assert_eq!(repaired.len(), 1);
        assert_eq!(repaired[0].role, "user");
    }

    /// tool 消息没有调用 id 时同样无法对应任何调用。
    #[test]
    fn tool_message_without_id_is_dropped() {
        let mut orphan = text("tool", "stray");
        orphan.tool_call_id = None;
        let repaired = repair_interrupted_history(vec![text("user", "hi"), orphan]);
        assert_eq!(repaired.len(), 1);
    }
}
