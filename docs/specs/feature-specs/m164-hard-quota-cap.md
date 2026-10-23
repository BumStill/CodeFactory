# M164（紧急）：GPT 留余量上限必须是硬上限（用户 10-23 明确要求）

## 用户要求（10-23 23:1x）
「忙时可以用 GPT 来跑任务，但是别超过我定的那个上限」——上限（默认 80%，五小时与周窗口）是硬约束。
## 现状证据
- `src-tauri/src/agent/quota_cap.rs:207` `CapBreach::is_hard_exhausted` 只在 100% 时为真；M152 把 80% 当「偏好」，无其它可用端点时继续用 GPT → 会越过用户上限。
- 10-23 10:01 会话提示「GPT 本五小时窗口已用 80%/81%，为给你留余量已切到 OpenRouter，约 14:10 恢复」——OpenRouter 无凭据，切过去必然失败（日志 AUTH_MISSING）。
- 10-23 会话 f3ff7831 10:40 进入 waiting_wake（等 DeepSeek 12:00 窗口），之后到 23:05 都没被唤醒。
## 需求
- R1 达到用户上限（cap_percent，五小时/周任一）即视为不可用：不再在该端点开新请求；正在进行的单次请求可完成。M152 的「软偏好继续用 GPT」分支删除（修改 M152 已批准规格，PR 中写明用户 10-23 的决定）。
- R2 选路候选只含**有凭据且在允许时段且未达上限**的端点；无凭据端点不进入候选、不出现在切换提示里、不产生 AUTH_MISSING 错误日志。
- R3 全部不可用 → waiting_wake，唤醒时间 = 最早的（GPT 窗口重置时间, DeepSeek 下一个允许时段开始）；到点必须真的唤醒并续跑（修 f3ff7831 未被唤醒的问题，查清是调度丢失还是重启后未恢复定时器），界面一句人话：「GPT 已到你设的 80% 上限，DeepSeek 14:00 前不可用，约 HH:MM 自动继续」。
- R4 设置页可见/可改上限（已有 subscription_quota_caps 则复用），默认 80%。
- R5 测试（先失败）：GPT 81% + DeepSeek 窗口外 + OpenRouter 无凭据 → waiting_wake（不调用 GPT、不尝试 OpenRouter）；GPT 79% → 用 GPT；到 DeepSeek 窗口开始时自动唤醒（跨进程重启后定时器仍生效）。
## 约束
- task-template；合成 fixture；授权交付（ceiling: merged）。

Release-Urgency: immediate

## 实施说明与需求追踪

用户 2026-10-23 的决定取代 M152 R1/R4 的软偏好语义，不是待批准的 Spec Amendment。

| Req ID | 验证边界 | 最低证据 |
| --- | --- | --- |
| R1 | 每次 POST 前准入，五小时/周上限 | 阈值边界与无备选测试先红后绿 |
| R2 | 路由规划、准入及切换通知 | synthetic 凭据缺失候选不会成为目标 |
| R3 | thin 等待持久化、到期恢复 | 数据库重开与到期续跑测试 |
| R4 | 复用设置与 dispatch | 默认 80%、部分更新与现有设置测试 |
| R5 | 组合情形与恢复 | 集成测试、全量 Rust/前端测试及构建 |

Applicable Harnesses：Spec、Compatibility、Observation、Release（仅合并交接，不发版）、AI Collaboration；如修改设置文案则追加 Viewport。

真机验收由编排方发版安装后完成：设置两个窗口上限；使 GPT 达到上限且 DeepSeek 时段外，确认不向 GPT/OpenRouter 发新请求、显示自动继续时间；重启应用并等到 DeepSeek 窗口开始，核对会话续跑及审计记录。会话内使用 synthetic fixture、集成测试与 headless 真浏览器，不读取生产会话内容。
