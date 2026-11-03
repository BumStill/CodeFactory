# M217：M212 的每日花费上限在真机上没有生效（只接了一半）

## 现场（2026-10-31，v1.91.69）
- 按单价算，DeepSeek 当天花费已到约 ¥10.04（上限 ¥10）。07:11Z 时 deepseek 路由仍然是 `succeeded`，没有出现 `daily_spend_cap` 跳过记录，会话继续花钱。
- 根因（读 origin/main 确认）：
  1. `SpendLedger` 只存在内存里，持久化时只调用了 `ledger().record_estimated`，`model_usage_events.estimated_cost_cny` 列从来没人写（当天 709 行全是 NULL）。应用一重启（今天 12:07、13:16 各装了一次新版）账本就清零，上限形同虚设。
  2. R7（余额）：`GET /user/balance` 只有解析函数和测试，生产代码里没有任何地方调用 `observe_balance`，真实花费和低余额提醒都没有接上。
  3. R9：dispatch 没有 `usage_status` 操作（`unknown variant usage_status`），`get_quota` 里也看不到 DeepSeek。
- M212 的单测全绿，可真机路径从头到尾没接通。验收时必须走真实应用，而不能只跑单测。

## 要求
- R1：每条用量落库时，同时写入 `estimated_cost_cny`。启动时（以及跨过本地 0 点时）用「今天本地 0 点以来的 SUM(estimated_cost_cny)」重建账本，保证重启后不清零。历史上为 NULL 的行，按当前单价从 token 数补算后计入（只在内存里补算，不改旧行也可以，但必须计入）。
- R2：把余额读取真正接上。DeepSeek 端点在应用启动时、每次路由决策前（加 60 秒缓存）、每回合结束时，用应用自己保存的凭据调 `GET /user/balance`，再调用 `observe_balance`。今天的开盘余额要落库，重启后不能丢。读取失败时退回单价估算，不得阻断。凭据绝不能出现在日志或输出里。
- R3：dispatch 增加只读的 `usage_status` 操作，并把 DeepSeek 的每日花费并入 `get_quota` 的结果。字段包括：已花、上限、剩余、来源（余额或估算）、余额、低余额标志、重置时间。不得包含凭据。
- R4：超线后，下一次路由必须产生一条 `daily_spend_cap` 跳过记录，会话转为 waiting_wake，唤醒时间是次日本地 0 点，界面上显示一句看得懂的话。
- R5：集成测试覆盖「写入用量 → 重建应用（新的 ledger 和设置，同一个 DB）→ 路由被拦」这条链路，不能只测内存 ledger。

## 验收（真机，必须）
- 装新版后执行 `codefactory --dispatch` 的 `usage_status`：已花金额与当天 model_usage_events 按单价的合计相符（误差 ≤5%），能读到余额时以余额差为准。
- 重启应用后再查一次，数字不清零。
- 把上限临时调低到略低于当前已花金额（`set_quota_cap` 或设置项），触发一次 DeepSeek 回合：出现 `daily_spend_cap` 跳过，会话进入「等待到明天 0 点」。验证完把上限改回 ¥10。

Release-Urgency: immediate

## Implementation Notes
- Spec / Compatibility / Observation / Release / AI Collaboration Harness：覆盖 SQLite 迁移、历史 NULL、重启、跨日、余额缓存与失败降级、dispatch 字段及路由跳过；发版和真实安装版验收由编排方另行完成。
- R1/R5：真实 migrated SQLite 写入、重建独立 ledger 和 Settings，再断言预算阻挡与午夜重置。
- R2：合成余额响应检查持久化开盘余额、充值、缓存与故障降级，不使用真实凭据作 fixture。
- R3/R4：公共协议解析、前端 dispatch 转发、实际路由等待测试；复用既有预算等待文案，不新增布局。
- 真机验收由编排方发版后完成：执行 usage_status、核对当日金额、重启复查、临时降低设置中的日限额触发等待并恢复 ¥10。
