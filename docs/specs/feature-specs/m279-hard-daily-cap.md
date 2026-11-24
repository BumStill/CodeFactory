# M279：日预算成为真正的硬上限（美元账本重启不归零 + 请求前预留）

## 问题

1. 美元账本 `openai_api::UsdSpendLedger` 只存在内存里。启动时 `daily_spend_live::rebuild`
   只重建人民币账本，应用重启后当天美元花费从 0 重算，上限形同虚设。
2. 上限检查只比较「已记账花费 ≥ 上限」。在途请求尚未写回用量，因此并发请求可以同时
   通过准入；11-11 两单并发在 ¥9.56 时双双放行，最终 ¥10.51 / 上限 ¥10。

## 需求

- **R1 启动重建美元账本**：与人民币账本同一个 `spend-rebuild` 步骤，从
  `model_usage_events` 当天 `openai` / `openai-*` 事件按 `estimated_cost_usd` 重建；
  该列为空时按模型价表 `estimate_cost_usd` 估算。未知模型按保守价估算，不记为免费。
- **R2 请求前预留**：发请求前按「输入 token 估算 + 本次 `max_tokens` × 输出单价」预留。
  只有「已花 + 在途预留 + 本次预留 ≤ 上限」才放行；否则不发该请求。请求结束用实际
  用量结算并释放预留；预留检查与写入在同一把锁内完成，两单并发不能同时通过。
  人民币与美元两条账本都要。进程崩溃/重启后内存预留自然失效；进程内超时预留
  （10 分钟）也会被自动回收。
- **R3 不改变真值来源与既有判定**：DeepSeek 余额快照仍是人民币真值来源，预留只在两次
  快照之间起作用；不改上限数值、不改路由顺序、不改任何权限判定。
- **R4 用量界面**：照常显示已花/上限，可附「在途预留」；本次不新增必填字段。

## 实现要点

- `quota_cap::daily_spend::SpendLedger` 与 `openai_api::UsdSpendLedger` 各自在同一个
  `Mutex` 保护的条目内维护 `reserved` 与 `reservations`，提供
  `try_reserve` / `settle_reservation` / `release_reservation`。
- `model_transport` 在发出 POST 前预留，收到 usage 后按实际 token 结算；提前返回或
  流中断时由 RAII guard 释放预留。
- 预留金额在人民币侧用 `PriceTable::estimate_cny(input, 0, max_tokens)`，美元侧用
  `estimate_cost_usd(model, input, 0, max_tokens)`；输入 token 以请求体字符数保守估算。

## 测试

- 重启：写入当天 openai 事件 → 新账本 `rebuild_usd` → `usage_status` 的 `spent_usd`
  等于事件之和。
- 并发：账面 ¥9.6 / 上限 ¥10，两单各预留 ¥0.5，只有一单拿到预留。
- 结算：预留大于实际时，结算后已花只增加实际值，预留归零。

## 验收（真机）

重启应用后 `usage_status` 美元花费不归零；两单并行时 DeepSeek 当天花费不超过 ¥10。
