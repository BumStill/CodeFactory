# M274：OpenAI API（Pro 会员额度）接入、计费、上限与优先级

## 背景
用户的 OpenAI Pro 会员附送了 API 额度。CodeFactory 之前只有两条 OpenAI 相关路径：
`chatgpt`（订阅 OAuth，按窗口限额、不按 token 计费）和 `openrouter`（第三方转发）。
直接使用赠送额度的 **OpenAI API**（`https://api.openai.com/v1`，按 token 以美元计费）
没有预设条目、没有成本记账、没有金额上限，所以「额度在哪、今天花了多少、会不会一夜花光」
都答不上来。

## 需求与实现位置

| Req | 要求 | 实现 |
|---|---|---|
| R1 | 预设可见的 OpenAI API 端点 | `Settings::default()` 与 `Settings::ensure_openai_api_preset()`（`src-tauri/src/config/settings.rs`）：`openai` 条目，`base_url=https://api.openai.com/v1`，`key_ref=codefactory.endpoint.openai`。老配置在 `load()` 时补入，**不覆盖**用户改过的同名条目。缺 key 时该路由被跳过并记录原因（M86 既有行为），不会静默失败。 |
| R2 | 每次调用按量计成本，缓存输入按缓存价，未知模型不当免费 | `src-tauri/src/agent/openai_api.rs`：`ModelRate::estimate_usd`（缓存命中按 `cached_input` 计价，且不超过输入总量）、`rate_for_model`（最长前缀匹配）、`CONSERVATIVE_RATE`（未知模型回落到价目表中最高价）。`commands/costs.rs` 在记账入库后把同一事件换算成美元写入 USD 账本。 |
| R3 | 可设美元日上限并优先让位 | `UsdDailyCap { enabled, daily_cap_usd }`（默认开启、$3/天，可用 `Settings.openai_api_daily_cap` 改）。`cap_wait_cause()` 达到上限即返回 `WaitCause(DailySpendCap)`；`agent/failover.rs::unavailable_cause` 把它并入让位原因，**不**受 `ignore_windows` 影响（钱不是时段）。 |
| R4 | 路由优先级可调、免费额度优先 | `Settings.endpoint_route_order`（默认 `chatgpt → openai → deepseek → openrouter`）+ `Settings::ordered_endpoint_names()`（只返回真实存在的端点）。 |
| R5 | 状态可见：今日花费/上限/重置时间，且标明估算 | `EndpointSpendReport.usd_statuses`（`quota_cap.rs::usd_spend_statuses`）给出 `spent_usd / cap_usd / remaining_usd / reset_at_ms / cap_reached / enforcing / estimated / rate_card_date`；`UsdSpendStatus::headline()` 即「OpenAI API 今日 $0.42 / $3.00（估算）」。 |
| R7 | 单条路由失败不毁整轮 | 有其它候选时按让位原因切换；无可用路由时给出一句人话（`cap_notice()`：「OpenAI API 今日预算已用完，明天 0 点后继续」）。 |

## 价目表口径
`RATE_CARD_DATE = 2025-09`，单位 **USD / 1M tokens**，来源为 OpenAI 公开价目表
（input / cached input / output 三档）。价目表是常量表，随官方调价改这一处即可；
查不到的模型按表内最高价保守估算，不允许出现「0 成本」的未知模型。

## 验证
- `agent::openai_api::tests`（6 条）：按模型与 token 数计价、缓存价不超过输入、未知模型非零、
  跨日重置与达到上限即让位、禁用上限只记账不拦截、预设/上限/预留顺序。
- `agent::failover::tests::openai_api_usd_cap_yields_to_the_next_candidate`：
  API 路由超预算后让位，下一条 DeepSeek 候选不受影响，`ignore_windows` 不能绕过金额上限。
- 相邻回归：`daily_spend`（12 条）、`config::settings`（29 条）全绿。

## 未完成 / 边界
- R4 的**预留顺序已持久化并有测试**，但自动回退链的组装点（`commands/agent.rs` 侧把
  候选塞进 `RouteCandidatePlan` 的地方）尚未改为消费 `ordered_endpoint_names()`；
  因此「openai 自动进入回退链」这条行为还没有端到端证据，只能通过显式选择该端点使用。
- 花费是本地 token 估算，不是账单；上游调价或缓存计费差异会让数字与发票有偏差（界面上标注「估算」）。
- 额度余额（剩余赠送额）来自 OpenAI 账单接口，本轮没有接入。
