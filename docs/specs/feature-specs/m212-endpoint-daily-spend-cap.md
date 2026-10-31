# M212：按端点的每日花费硬上限（人民币）

Release-Urgency: default

## 背景

用户 10-30：「DeepSeek 每天设个上限，不要用太多，差不多闲时用 10 块人民币的量就行」。

现有 `commands/costs.rs` 只回答美元成本口径的问题：

- 没有人民币预算概念；`model_usage_events.estimated_cost_usd` 也可能为空
  （DeepSeek 的 usage 里没有单价）。
- 余额耗尽后 API 返回的不是 `Retryable`，而是 `PROVIDER_FATAL`；M212 的现场
  日志里它是 10-29 烧掉 43M 输入 token、余额见底之后的一整天失败。
- 用户要的是**每天不超过 ¥10**，而"重试"和"余额不足"是两件事：
  前者是技术恢复，后者应当是一个明确的、会自己到点消失的等待理由。

## 需求

| ID | 要求 |
| --- | --- |
| R1 | 按端点配置每日人民币上限（默认 ¥10）与单价表（元/百万 token） |
| R2 | 记录每个端点的今日花费：优先用供应商余额快照，其次用自己按单价算的估算；写进库要能区分口径 |
| R3 | 路由前判定：预算用完且**还有其它可用模型**时改走其它模型；只剩它时才进入等待，唤醒时间取"次日 00:00"与"使用时段下次开放"的较晚者 |
| R4 | 上限只挡下一个请求，不中断已在飞的一个请求 |
| R5 | 用法页 / 会话里显示「DeepSeek 今日 ¥x / ¥10」；预算用完显示「DeepSeek 今日预算已用完，明天 0 点后继续」 |
| R6 | DeepSeek 带内置默认值（¥10/天、按 10-30 真实账单反算的单价），开箱即用 |
| R7 | 余额快照优先于估算；充值是入账，不是负花费 |
| R8 | 余额低于告警线先提示，而不是等余额为 0；余额不足的请求错误要能读懂 |
| R9 | 带凭据轮询余额，并给调用方一个不含凭据的只读入口 |

## 实现

### 账本（`src-tauri/src/agent/daily_spend.rs`）

- `PriceTable`：命中缓存输入 ¥0.1 / 未命中输入 ¥0.8 / 输出 ¥2（每百万 token，
  按用户 10-30 的账单反算）。命中量从输入量里扣掉，不重复计费——这正是早期
  看门狗把花费高估约 3 倍的原因。
- `DailySpendConfig`：`enabled` / `daily_cap_cny` / `low_balance_warn_cny` / `prices`。
  `cap <= 0` 或 `enabled = false` 表示"不设上限"，仍然记账但不拦路由。
- `SpendLedger`：进程内、按端点、按**本地日**（Asia/Shanghai，与应用其它用量口径
  一致）累计。日切自动清零，昨天的数字不会带过来。
- 口径优先级：有余额快照时 `花费 = 当日首个余额 + 当日入账 - 当前余额`，否则用
  单价估算，并在展示时标注「（估算）」。

### 记账入口（`commands/costs.rs::record_usage_event`）

- 入库时同时写 `estimated_cost_cny`（新增列，与 `estimated_cost_usd` 并存，不混币种）。
- 原来 `cost_source = unknown` 且能定价时，升级为 `configured_price`；该来源禁止携带
  任何美元字段（校验里显式拒绝），所以不会和 `provider_actual` 混淆。
- 只有在插入真的落地（`rows_affected == 1`）时才累加到账本，持久化回调重放不会
  把同一笔花费算两次。

### 路由（`agent/failover.rs` + `agent/usage_windows.rs`）

- `WaitCauseKind::DailySpendCap`：与订阅额度上限同级，属于"会自己好"的等待理由，
  句子是「DeepSeek 今日预算已用完，MM-DD HH:MM 后继续」。
- `admit_availability` 里对每个候选同时看时段/额度与每日预算；被挡时记
  `daily_spend_cap:{spent,cap,resets_at_ms}` 跳过码，等待说明里写明原因与恢复时间。
- 预算用完与时段未开放同时成立时取**较晚**的时间，避免提前醒来空转。
- 快速通道（无时段、无订阅额度）现在也会检查每日预算，否则内置默认值不会被检查。

### 只读入口

`commands/costs.rs::get_endpoint_spend_status`（Tauri 命令，已注册）返回
`{ currency, statuses[] }`，每项含 `spent_cny / cap_cny / remaining_cny / source /
reset_at_ms / cap_reached / balance_cny / low_balance`，不含凭据、`key_ref` 或
`base_url`。

## 验证

- `src-tauri/src/agent/daily_spend.rs` 单元测试：单价与缓存命中去重、Asia/Shanghai
  日界（00:00 +08 = 16:00Z 前一日）、跨过上限后阻塞且次日 00:00 解除、余额口径优先
  于估算、充值是入账、低余额与"余额不足"错误识别、无单价端点不产出价格、被挡端点
  让位给可用端点。
- `src-tauri/src/commands/costs.rs::m212_daily_spend_tests`：只读报告给出的上限、重置
  时间与展示句子。
- 命令：`node scripts/cargo-shared.mjs test --manifest-path src-tauri/Cargo.toml --lib m212 -- --nocapture`
  与 `... --lib daily_spend`。

## 尚未完成（后续增量）

- R5 的前端展示：后端已给出 `headline()` / `cap_notice()` / `low_balance_notice()`
  与 `get_endpoint_spend_status`，用法页与会话气泡还没接。
- R7 的**定时**轮询余额：解析（`parse_balance_cny`）、入账（`observe_balance`）、
  低余额判定已完成并有测试；把 `GET /user/balance` 挂到启动/定时任务还需要走
  `crate::secrets::get_key` 取凭据的调用点，尚未接线。
- R8 的余额不足错误文案已提供判定函数，尚未并入供应商错误分类。
