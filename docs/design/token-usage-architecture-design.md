# Token 用量与预算架构设计

## 1. 设计选择

首版维持 `Tauri + Rust + SQLite + React` 本地架构，不引入外部遥测服务。计量真相源从“最终回答成本行”升级为“每一次 Provider 请求的规范化 Usage 事件”。消息表继续负责会话重放，不承担统计真相源；现有 `cost_entries` 在兼容期保留只读，不直接重命名或破坏。

```text
OpenAI/OpenRouter/ChatGPT/Anthropic/local provider response
  -> UsageNormalizer
  -> ModelUsageRecorder (idempotent, every request attempt)
  -> model_usage_events (SQLite, UTC)
  -> model-usage-recorded Tauri event
  -> local-day aggregations
  -> Welcome / Workspace footer / Settings usage map
  -> session/task/eval log deep link
```

## 2. 当前事实与缺口

- `record_cost_entry` 目前只在 AgentLoop 的无工具最终轮执行；包含工具调用的中间模型轮次未进入 `cost_entries`。
- OpenAI-compatible assistant message 会保存 `prompt_tokens` 和 `completion_tokens`，但聚合命令不读这些逐轮数据。
- `Usage` 当前只有 prompt/completion/total；reasoning、cached 与 Provider actual cost 尚未进入统一合同。
- `get_today_cost` 通过 UTC 日期字符串截取查询，却在 UI 展示为“今日”。
- 所有 Endpoint/模型统一按 input `$1/M`、output `$3/M` 估算，无法表达订阅、本地、实际或未知成本。
- `ContextUsageBar` 已监听 `token-usage-recorded`，但收到的是最终轮级漏计结果；`CostDashboardSection` 位于 Profile，职责和口径均需迁移。

因此，任何 UI 实现必须依赖新表和新聚合接口；不得在旧命令上叠加地图后宣称完成。

## 3. 规范化 Usage 合同

Provider adapter 在每次模型请求完成时产生 `NormalizedModelUsage`：

```text
request_id             每轮真实 Provider 请求的稳定本地 ID
attempt_id             同一轮的幂等键，首版与 request_id 一致
session_id             持久 session；anonymous 不落库
task_id                自主任务或子 Agent 作业关联，可空
surface                interactive | autonomous | subagent | eval
provider / endpoint / model
input_tokens / output_tokens
reasoning_tokens       缺失为 0，且视为输出或 Provider 定义的子维度
cached_tokens          缺失为 0，视为 input 子维度
actual_cost_usd        可空
estimated_cost_usd     可空
cost_source            provider_actual | model_price_estimate | subscription | local | unknown
source                 provider_usage | backfill_message | legacy_cost_entry
created_at             规范化 UTC 时间
```

规范化规则：

- 总 Token 始终为 `input_tokens + output_tokens`；reasoning/cached 只作为拆分，不再次加总。
- Provider 同时返回 total 时，记录但校验差异；不能用 total 和分项相加。
- Usage 缺失时不猜 Token；记录应用诊断日志，不插入虚构的 0-token 成功事件。
- Provider 返回负数、非有限成本、明显不一致字段时 fail closed：拒绝该 Usage 事件并留下无敏感信息的诊断。
- 每个真实网络重试使用新 `attempt_id` 并计量；UI 重放、stream event 重送和恢复使用同一 attempt，幂等忽略。
- anonymous session 只在当前进程内累计临时用量，永不写 SQLite、预算累计或历史地图。

## 4. SQLite 模型

新增表 `model_usage_events`：

| 字段 | 约束/用途 |
| --- | --- |
| `id TEXT PRIMARY KEY` | 本地事件 ID |
| `request_id TEXT NOT NULL` | 逻辑请求关联 |
| `attempt_id TEXT NOT NULL UNIQUE` | exact-once 幂等键 |
| `session_id TEXT NOT NULL` | 会话下钻 |
| `task_id TEXT NULL` | 作业关联；通过 `task_runs` 反查父会话 |
| `surface TEXT NOT NULL` | 执行入口白名单 |
| `provider/endpoint/model TEXT NOT NULL` | route 拆分 |
| `input_tokens/output_tokens INTEGER NOT NULL CHECK >= 0` | 主计量 |
| `reasoning_tokens/cached_tokens INTEGER NOT NULL DEFAULT 0 CHECK >= 0` | 可选子维度 |
| `actual_cost_usd/estimated_cost_usd REAL NULL CHECK >= 0` | 分离成本 |
| `cost_source TEXT NOT NULL` | 成本语义白名单 |
| `source TEXT NOT NULL` | live/backfill 来源 |
| `created_at TEXT NOT NULL` | 规范化为 RFC3339 UTC 毫秒格式 |

索引至少包含：

- `(created_at)`；
- `(session_id, created_at)`；
- `(surface, created_at)`；
- `(model, created_at)`；
- `attempt_id` 唯一索引；`request_id` 继续保留唯一约束，防止恢复重放重复。

预算配置保存在现有 settings 合同的 `usage_budget`：`daily_token_limit`、`monthly_token_limit`、`alert_thresholds=[0.5,0.8,1.0]`、`alerts_enabled`。预算单位仅为 Token；首版不以估算美元强制停机。

## 5. 记录时机与事务边界

### 5.1 OpenAI-compatible 路径

每轮 transport 完成并解析出 Usage 后，立即调用 recorder；记录必须发生在检查 `tool_calls.is_empty()` 之前。因此工具轮、completion recovery、最终轮均分别计量。assistant message 与 Usage event 使用同一个 `request_id/attempt_id` 关联；任一 UI event 失败不回滚 SQLite。

### 5.2 Anthropic 路径

扩展 Anthropic normalized response，将 input/output/cache 子维度交给同一 recorder。不得继续只把 token 用于 `ContextUsage` event 而不落库。

### 5.3 子 Agent、自主任务与 Evals

创建 AgentLoop 时显式传入 `surface` 和 `task_id`。不得通过标题、cwd 或模型名猜测入口。已有未传值调用只能落为准确的 `interactive`；当前自主任务由子 Agent 执行，因此计为 `subagent`，`autonomous/eval` 保留给对应直连执行路径。

### 5.4 事件刷新

SQLite 成功提交后发出 canonical `model-usage-recorded`，payload 仅含计量事件 ID，不含 secret 或原始请求。兼容期可同时发旧 `token-usage-recorded`，待所有消费者迁移后删除旧事件。

## 6. 聚合与 API

新增或替换为以下 Tauri command：

- `get_usage_dashboard(range_days, timezone_offset_minutes)`：当地日摘要、连续地图、入口拆分和 Top 会话。
- `get_usage_day_detail(local_date, timezone_offset_minutes)`：选中日期明细。
- `get_usage_budget_status(timezone_offset_minutes)`：日/月预算状态和新触发阈值回执。
- `get_session_usage(session_id)`：当前会话汇总。

日期查询不能用 `substr(timestamp, 1, 10)`。前端传 IANA timezone 能力稳定前，先传本机 `timezone_offset_minutes`，后端把当地 `[00:00, next 00:00)` 转成 UTC 半开区间查询。跨 DST 平台后续改为 IANA timezone；同一查询返回实际 `start_utc/end_utc` 供证据核对。

所有聚合响应返回：`data_status=complete|partial|unavailable`、`source_counts` 和可选 `missing_usage_count`。空数组不能自动解释为零用量。

## 7. 成本计算

- Adapter 已返回费用：写入 `actual_cost_usd` 和 `provider_actual`。
- ChatGPT OAuth/订阅 Endpoint：`cost_source=subscription`，实际/估算费用均为空。
- 本地 Endpoint：`cost_source=local`，实际费用为空；UI 可显示“本地”，不折入实际费用。
- 有版本化价格目录且用户启用估算：只写 `estimated_cost_usd` 和 `model_price_estimate`。
- 其他情况：`unknown`。

价格目录必须带 provider/model/effective_at/version，不能继续使用全局常量。价格变更只影响新事件；历史事件保存当时估算，不在查询时静默重算。

## 8. 兼容与迁移

迁移采用 additive、可回退路径：

1. 创建新表、索引与 settings 默认值，不修改或删除 `cost_entries`。
2. 对带 Usage 的历史 assistant message 按 message ID 生成确定性 `attempt_id=backfill:message:<id>`，插入 `source=backfill_message`；reasoning/cache/实际费用保持空。
3. 仅当某条 legacy `cost_entries` 无对应 message backfill 时，才允许插入 `source=legacy_cost_entry`；其 `cost_usd` 只映射为 `estimated_cost_usd`，绝不映射为 actual。
4. backfill 使用 `INSERT ... ON CONFLICT DO NOTHING`，可中断重跑；保存迁移版本和扫描/插入/跳过/冲突计数。
5. 新 UI 只读 `model_usage_events`；旧 `cost_entries` 保留一个发布周期作为回滚数据，不双表求和。
6. 回退旧版本时旧表仍可读；新表被旧版本忽略。确认发布稳定后另开迁移删除旧写入路径，不在本特性首个发布中删表。

历史地图必须显示“历史回填”提示；不能声称历史 reasoning、cache 或实际费用完整。

## 9. 深链、隐私与可观察性

- 高消耗会话只返回稳定 ID、标题、cwd basename、模型、入口、时间和计量，不返回 prompt、reasoning、工具参数或 secret。
- 点击条目优先进入 session；存在 task/eval/evolution job 时提供“查看作业日志”动作，复用现有 canonical route，不复制第二套日志。
- recorder 记录 route 和计量，不记录 API key、authorization、cookie、原始请求体、模型正文或 reasoning 内容。
- 聚合返回 `source_counts`、`missing_usage_count` 与 `data_status`，用于区分完整、历史回填和 Usage 缺失；更细的跨版本覆盖率遥测属于后续增强。
- 预算提醒保存在本机的 threshold receipt，键为 `period + threshold`，同一周期同一阈值只提醒一次；跨过多个阈值时合并提示。

## 10. 失败与恢复

- Usage 落库失败不得让已完成模型回复消失，但必须记录可见的“统计暂不可用”状态和本地诊断；不能静默展示较小总数。
- UI 查询失败保留上次成功数据并标记时间与“可能已过期”，不显示为 0。
- 迁移失败保持旧表和旧版本可启动；新 UI 显示 unavailable，不执行破坏性清理。
- 进程重启后聚合直接从 SQLite 恢复；未发出的 UI event 不影响最终查询真相。
- Provider 缺失 Usage 时继续完成会话，但增加 `usage_missing`；该请求不进入虚构计量。

## 11. 发布边界

本地 schema、单元测试、浏览器地图或 Dev App 均不是发布完成。必须验证迁移前后数据库、至少 OpenAI-compatible/Anthropic/ChatGPT 订阅/缺失 Usage 四类 route、真实工具多轮、重启、当地日界、双视口、锁屏 headless、PR+CI、Windows 安装包和 macOS 发布 DMG。完成精确发布产物验证前一律为 `not live`。
