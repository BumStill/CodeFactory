# M154 回合中途被杀后「点继续、瞬间失败」的断点续跑修复

## 现象

- 用户在只读演练里让模型跑 `bash sleep 60`，执行中途重启 / 关窗口 / 强杀进程。
- 重启后会话卡在「运行中」（幽灵回合），objective 状态是 `waiting_system`，
  `chat_turn_state` 是 `running`。
- 点「继续」或任何新消息都会在约 2 秒内失败，提示形如
  「同一个技术失败连续 3 次: agent loop stopped: agent loop error」，
  且 **不落库任何 assistant 消息**；连续 3 次后会话被判定为技术失败。

## 根因（两条，都在「续跑历史」这一层）

1. **续跑提示写成了 `system` 角色，追加在历史末尾**
   （`commands/chat.rs::push_thin_resume_prompt`）。
   ChatGPT / Anthropic 两条 wire 路径对 `system` 都是 **赋值而不是追加**：
   - `agent/model_transport.rs` 里 `"system" => instructions = content_to_text(...)`
   - `chat_messages_to_anthropic` 里 `"system" => system = ...`

   于是末尾那条几十字的续跑提示会把整段系统提示词（工具协议、完成门槛、
   objective 规则）整体覆盖；同时部分 provider 直接拒绝不在对话开头的 system
   消息。系统提示词被清空后的第一轮请求就是一个没有工具协议的请求，几秒内即
   失败，且失败发生在任何 assistant 消息落库之前 —— 与观察到的 2 秒 + 空消息
   完全一致。

2. **中断留下的残缺工具往返没有被修复**。回合在工具执行期间被杀会留下：
   - *悬空 tool call*：assistant 消息带 `tool_calls` 落了库，工具结果没落库。
     OpenAI Responses 要求每个 `function_call` 后必须有 `function_call_output`，
     否则报 `No tool output found for function call`。
   - *孤立 tool 结果*：结果落了库，发起它的 assistant 消息没落库。所有兼容
     OpenAI 的 provider 都拒绝没有对应调用 id 的 tool 消息。

   用户点「继续」时新 user 消息又排在悬空调用之后，请求形状永远是坏的 ——
   这就是「连续 3 次、每次 2 秒、同一条错误」。

3. **错误信息被丢弃**：续跑失败分支把 `settle_outcome` 的 stop label 写死成常量
   `"agent loop error"`，真实错误只进了 GUI 看不到的 tracing。用户和后续排查
   都拿不到线索。

## 修复

| 位置 | 改动 |
|---|---|
| `crates/agent-loop/src/history_repair.rs`（新增） | `repair_interrupted_history`：幂等修复历史 —— 悬空 tool call 补一条明确的「结果未知」tool 结果；孤立 tool 结果（含无 id 的）丢弃；非首条 `system` 降级为 `user`。干净历史原样通过。 |
| `crates/agent-loop/src/run.rs` | 在唯一的 loop 入口 `run_agent_loop` 解构后立即调用修复；断点续跑、用户「继续」和系统自动自愈三条路径同时受益，不需要用户做技术性操作。 |
| `commands/chat.rs::push_thin_resume_prompt` | 续跑提示改为 `user` 角色，不再覆盖系统提示词。 |
| `commands/chat.rs` 续跑失败分支 | stop label 带上真实错误首行（截断 160 字符，空则回退旧常量），界面能直接看到被拒绝的原因。 |

## 验收

- `node scripts/cargo-shared.mjs test -p codefactory-agent-loop history_repair`
  —— 6 个用例：干净历史幂等通过、末尾 system → user、悬空调用补「结果未知」、
  悬空调用在 user 之前闭合、孤立结果丢弃、无 id 的 tool 消息丢弃。
- `node scripts/cargo-shared.mjs check --manifest-path src-tauri/Cargo.toml`
  —— 主 crate 类型检查通过。
- 主路径证据：同一个「中途被杀 + 点继续」场景，修复后续跑历史最后一条是
  user 角色，且没有任何 `system` 消息落在历史中间。

## 非目标

- 不改变工具权限、审批、objective 语义。
- 不改 provider 的请求体形状；修复发生在历史层，对全部 endpoint 一致生效。
