# M170：重启续跑后对话里要有「上一轮被中断」说明

## 背景

M157 之后，回合中途被杀掉的进程会在重启时被扫描到并续跑，用户最后还是能拿到答案。
但真机验证（`verify-liveness.sh`：`bash sleep 60` 执行中重启 App）连续多个版本
（v1.91.29–v1.91.34）都显示同一个缺口：**对话里没有任何一句说明上一轮被中断过**。
用户看到的是「用户提问 → 隔了很久 → 一段答案」，中间那次进程退出被完全隐藏。

模型侧其实一直收到一条续跑提示（`thin_resume_message`），但那是内存里追加的
`user` 回合，不落库、不进对话（M154 R：wire 上必须是 user，避免 system 覆盖系统提示词）。
所以「模型知道」而「用户不知道」。

## 需求

| ID | 要求 | 验收 |
| --- | --- | --- |
| CF-M170-R1 | 回合因进程退出被中断、随后续跑时，对话里必须出现一条简短人话说明：被中断的是哪一步、结果未知、系统已重新执行/继续，且不含内部词汇（`thin`、`objective`、`completion_state`、续跑提示原文等） | Rust 单测断言文案 + 真机/跨进程 smoke 断言该行存在 |
| CF-M170-R2 | 说明作为**可见的系统消息**持久化，重启/重放后仍在；同一场景刷新页面后仍然只出现一次 | `messages` 行 `role='system' AND completion_state='turn_notice'`，前端 `MessageList` 已把该组合渲染为居中运行时提示 |
| CF-M170-R3 | 先失败后通过：先写「续跑后必须有一条可见中断说明」的断言并看到失败，再修；把 `verify-liveness` 的 interrupt notice 断言加进 M157 的跨进程 smoke | 失败记录 + `history_session_smoke` 的 `verify-liveness` 阶段 |
| CF-M170-R4 | 只出现一次、且不进模型上下文：重复续跑（同一中断去重）不追加新行，重放历史时该行被排除 | 去重 marker + `replayable_history` 现有排除逻辑；幂等断言 |

## 设计

复用了仓库已有的「运行时提示」通道，而不是新造 UI：

1. **写入**：`storage::persist_turn_notice_once(pool, session_id, marker, content)`
   —— `gate_events` 里 `INSERT OR IGNORE` 一个 `notice_marker`，命中即返回 `false`，
   未命中才插 `messages(role='system', completion_state='turn_notice')`。语义与
   agent-loop 的 `persist_gate_message_once` 的 `turn_notice` 分支一致。
2. **文案**：`commands::chat::interrupt_notice_text(tool_name: Option<&str>)`。
   只用人话描述「哪一步 + 结果未知 + 已自动继续」，不复述工具原始参数（不泄漏
   JSON、路径、命令原文）。
3. **触发**：`commands::chat::persist_resume_interrupt_notice(pool, session_id, key)`
   在 thin restart 续跑入口（`resume_thin_running_session`）里紧跟
   `push_thin_resume_prompt` 调用。那条续跑提示只进模型上下文、不落库，所以
   用户侧必须单独写一行。key 用 `thin-restart:<session>:<interrupt_at>`：同一次
   中断复用同一 key，天然幂等；新的中断才是新的 key，也就允许出现第二条说明。
   objective 续跑入口不写这条说明——那里未必是进程退出，写成「被重启中断」
   会说假话。
4. **可见性**：`MessageList` 只把 `role='system'` 且 `completion_state='turn_notice'`
   的行放进正文（`src/components/MessageList.tsx`），因此不需要新增渲染分支。
5. **不污染模型上下文**：`agent/mod.rs::replayable_history` 已经排除
   `turn_notice`；`latest_user_instruction` 只认 `completion_state IS NULL` 的
   user 行，因此这条说明不会变成下一轮的用户目标。

## 验收分工

- 自动化：Rust 单测（文案 + 落库 + 幂等）、`history-session-smoke verify-liveness`
  跨进程阶段（独立进程 seed 中断状态、独立进程续跑并断言恰好一条可见说明）。
- 真机：`~/.codefactory/agent-tools/verify-liveness.sh` 的
  `interrupt notice:` 计数必须 ≥1（该行为 M157 既有断言行，M170 使其从 0 变 1）。
