# M164：GPT 留余量上限必须是硬上限

## 用户决定（2026-10-23）
「忙时可以用 GPT 来跑任务，但是别超过我定的那个上限」。本决定覆盖 M152 的软偏好规则；不再允许无备选时继续使用达到 cap 的 GPT。

## 已批准任务原文
- R1 达到用户上限（cap_percent，五小时/周任一）即视为不可用：不再在该端点开新请求；正在进行的单次请求可完成。M152 的「软偏好继续用 GPT」分支删除（修改 M152 已批准规格，PR 中写明用户 10-23 的决定）。
- R2 选路候选只含**有凭据且在允许时段且未达上限**的端点；无凭据端点不进入候选、不出现在切换提示里、不产生 AUTH_MISSING 错误日志。
- R3 全部不可用 → waiting_wake，唤醒时间 = 最早的（GPT 窗口重置时间, DeepSeek 下一个允许时段开始）；到点必须真的唤醒并续跑（修 f3ff7831 未被唤醒的问题，查清是调度丢失还是重启后未恢复定时器），界面一句人话：「GPT 已到你设的 80% 上限，DeepSeek 14:00 前不可用，约 HH:MM 自动继续」。
- R4 设置页可见/可改上限（已有 subscription_quota_caps 则复用），默认 80%。
- R5 测试（先失败）：GPT 81% + DeepSeek 窗口外 + OpenRouter 无凭据 → waiting_wake（不调用 GPT、不尝试 OpenRouter）；GPT 79% → 用 GPT；到 DeepSeek 窗口开始时自动唤醒（跨进程重启后定时器仍生效）。

## Requirements Traceability / 测试矩阵
| Req | 最低证据 |
|---|---|
| R1/R2/R5 | 真实准入函数：81%、80%、79%、周窗口、忽略时段不绕过 cap、缺凭据预过滤 |
| R3/R5 | settle_error → SQLite 调度持久化 → 关闭重开数据库 → drain_due_wakeups_at → WakeSink 续跑回调；最早可用时间断言 |
| R4 | 复用 SubscriptionQuotaSection、desktopDispatch.quota 和现有 headless 上限设置验收 |

## Applicable Harnesses
Spec、Compatibility（旧设置/薄会话）、Observation（等待原因/真实持久唤醒）、Release（仅合并，真机由编排方发版安装后验证）、AI Collaboration。设置 UI 不作布局变更，沿用既有验收页。

## Implementation Notes
合成 fixture，不读取真实会话内容或凭据。每次 POST 的模型可用性准入是最终权威；路由快照保留不可用条目的跳过原因，绝不将其交给传输。模型等待必须注册到现有持久 wakeup_schedules，不可只写人类可读的 wake_source。

## 真实应用验收分工
真机验收由编排方发版后完成：设置五小时/周上限为 80%；复现 GPT 81%、DeepSeek 窗口外、OpenRouter 无凭据；确认等待且没有 GPT/OpenRouter POST；重启应用，窗口开始时自动继续且不要求用户回复。
