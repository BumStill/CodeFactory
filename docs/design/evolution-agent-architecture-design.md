# Evolution Agent 架构设计

## 1. 设计选择

CodeFactory 首期保持 `Tauri + Rust + SQLite` 本地架构。演示方案中的 OTel `gen_ai.*` 作为字段语义参考，不引入独立遥测服务。结构化数据留在本机；远程二次分析后续单独 opt-in。

## 2. Phase 0 数据路径

```text
Model round
  -> assistant message + tool declarations
  -> normalized tool_calls INSERT(status=pending, redacted args)
  -> permission / hook / dispatch
  -> normalized tool_calls UPDATE(done|error|denied, redacted result/error, duration)
  -> session/task postmortem
  -> learning_events pending
  -> human accept/reject
```

`messages.tool_calls` 继续用于模型对话重放，但持久副本先脱敏且不做轨迹限长；规范化 `tool_calls` 是分析、Evidence 与审计真相源，使用限长摘要。两者职责不同，不能删除前者或继续让分析读取空表。

## 3. Phase 0 合同

### `tool_calls`

沿用现有表：

- `id`：`session_id + provider_tool_call_id` 的稳定复合标识，避免跨 session 冲突。
- `message_id`：声明该调用的 assistant message。
- `tool_name`、`arguments`：入库前按敏感 key 和常见 token 模式脱敏并限长。
- `status`：`pending | done | error | denied | legacy`。
- `result`：成功输出的脱敏限长摘要。
- `error`：失败/拒绝的脱敏限长摘要。
- `duration_ms`：真实执行耗时；未执行的拒绝路径为 `0`。
- `created_at`：工具声明时间。

分析只把 `done` 与 `error` 计入工具可靠性；`denied` 不是工具故障，`pending/legacy` 不进入失败率分母。

### 兼容

- `ensure_schema` 必须创建缺失的 `tool_calls` 表和索引，兼容旧数据库。
- 历史 message JSON 不推断不存在的失败状态或耗时；如后续回填，状态只能标记 `legacy`。
- 新路径保持脱敏后的 `messages.tool_calls`，旧会话仍能重放。

## 4. 脱敏合同

入库、Evidence 和后续候选证据复用同一套 redactor：

- 递归屏蔽 `api_key`、`token`、`password`、`secret`、`authorization`、`cookie`、`credential` 等 key。
- 屏蔽 Bearer、OpenAI/GitHub 常见 token、私钥块和 URL userinfo。
- normalized arguments、result、error 分别限长；provider replay message 只脱敏、不截断，避免破坏长参数和上下文。
- 用户主动输入继续遵循现有聊天历史保留语义；系统派生的 assistant、reasoning、tool result、tool declaration、normalized trace 与 Evidence 不复制命中的敏感值。
- `reasoning_content` 不进入 Evolution 分析或 Evidence；现有 provider 重放副本仅做脱敏持久化。
- anonymous AgentLoop 在任何 recorder 调用之前返回，保持零 DB 写入。

## 5. 普通聊天 post-mortem

`run_postmortem` 优先使用 task outcome；没有 `task_runs` 时，使用同 session 的有限用户/助手轮次摘要和规范化工具状态统计。摘要必须先脱敏和限长，不复制完整对话或工具结果。

空 session 返回空；有足够持久消息的 project/Quick session 不得仅因没有 task_run 而返回空输入。LLM 调用仍为 best-effort，不能阻塞正常聊天。

## 6. Evidence

Evidence Pack 优先读取真实 `tool_calls` 表，输出 tool name、脱敏 args/result/error、status、duration、message/session 关联。旧消息 JSON 只作为兼容 fallback，并同样脱敏。不存在的 `tool_call_records` 查询必须删除。

## 7. 后续目标架构

Phase 1 以后再增加：`agent_runs`、`agent_events`、`evolution_jobs`、`improvement_candidates`、`candidate_evidence`、`candidate_reviews`、`candidate_change_receipts`、`eval_cases/eval_runs`。候选状态采用 `draft -> pending_review -> approved/rejected -> materialized -> eval_passed/eval_failed -> pending_activation -> active`，所有写动作带 `expected_revision`。

## 8. 风险

- 双写不一致：规范化写入失败应使当前持久 Agent 路径显式失败或产生 dropped 计数，不能静默声称轨迹完整。
- 数据膨胀：只存脱敏限长摘要，不复制 token delta 与 reasoning。
- 错误归因：权限拒绝与 hook cancel 不计入工具故障。
- 文档漂移：每个阶段必须更新 `docs/self-evolution/README.md` 的真实状态。
