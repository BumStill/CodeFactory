# 订阅模型的用量上限可配置，到点自动换端点

## 背景与用户决策

订阅模型的额度由 CodeFactory 和用户本人共用。CodeFactory 要优先用订阅（已经付过钱的），
但不能把额度吃光，给用户留出余量。

ChatGPT Plus 订阅（Codex 通道）按「5 小时滚动窗口」加「每周上限」计量，按 token 折算额度。
2026-10-10 当天两个窗口都被 CodeFactory 的任务用光：早上 10:20 一次，下午 18:20 左右一次，
每个窗口大约 2700–3200 万输入 tokens。额度用光后，用户自己在 ChatGPT / Codex 上也没得用了；
任务也跟着卡住或失败。v1.84.0 的 U33 已经能在用光时自动回落，但仍是先用光再回落。

用户决定（2026-10-10）：「有 gpt 优先用 gpt，但是能不能每次五小时用到 80% 最多」——GPT 优先，
但每个 5 小时窗口最多用到 80%，剩下 20% 留给用户自己用。

## Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| CF-QUOTA-R1 | **知道用了多少**：对订阅类端点（当前是 ChatGPT），读取服务端给出的当前 5 小时窗口和每周窗口的已用比例与重置时间（与 Codex CLI `/status` 显示的同源，例如响应头或用量接口）。读不到时退回本地估算（按 token 类型与官方计费表折算），并标明是估算 | 测试：合成的服务端用量数据解析正确；读不到时退回估算 |
| CF-QUOTA-R2 | **可配置上限**：每个订阅端点可设「5 小时窗口最多用到 X%」「每周最多用到 Y%」，默认 5 小时 80%、每周 80%；在设置里可改，后台入口也能改（符合「可后台操控」默认要求） | 组件测试 + 后台入口测试 |
| CF-QUOTA-R3 | **到点自动换**：已用比例达到上限后，新的模型请求不再发给该端点，按 prefer 顺序改用下一个可用端点（例如 DeepSeek）；窗口重置、用量回落到上限以下后，新的回合自动回到订阅端点。进行中的一轮在下一次模型请求时切换，不重放已有副作用 | 集成测试：越过上限 → 改用回落端点；重置后 → 回到首选 |
| CF-QUOTA-R4 | **看得见**：会话里用大白话说明「GPT 本窗口已用 80%，为给你留余量已切到 DeepSeek，约 HH:MM 恢复」；设置或用量页能看到各订阅端点的当前用量、上限与重置时间 | 组件测试 + 真浏览器截图（light/dark）；文案通过内部词汇守卫 |
| CF-QUOTA-R5 | **不误伤**：上限只影响 CodeFactory 自己发出的请求；只有订阅端点受上限约束，按量付费端点不受影响；所有端点都不可用时如实说明，不空转消耗恢复次数 | 测试 |

## Applicable Harnesses

- Spec Harness：CF-QUOTA-R1..R5。
- Compatibility Harness：复用既有的 `resolve_route_plan` 路由与 `Settings`/`Endpoint` 配置，
  旧配置文件缺字段时回落到默认 80%/80%。
- Observation Harness：切换原因（哪个窗口、已用多少、上限多少、何时重置）可追溯，
  进入 route change reason 与会话 `turn_notice`。
- Viewport Harness：R4 的设置项与用量展示在 light/dark 下不遮挡、不溢出。
- AI Collaboration Harness：核心计量/决策逻辑与接线分角色实现与复核。

## 测试矩阵

- 用量 79% / 80% / 95% / 100%，以及重置后。
- 服务端用量可读、读不到（退回估算）两种。
- 进行中的回合越过上限。
- 回落端点也不可用。

## Implementation Notes

## CF-QUOTA 真机返修（2026-10-14）：实时查询订阅剩余额度

### 背景与用户决策

用户要求（2026-10-14）：「要实时获取它的剩余额度，而不是 CodeFactory 自己的」。

真机证据（v1.88.2，2026-10-14 23:3x）：`get_quota` 返回 `five_hour_percent=0`、`source=服务端读数`，
但 `five_hour_resets_at` 仍是 14:05。该读数来自回复头 `x-codex-*-used-percent`，只在请求经过时更新；
过了重置时间后 `effective_ratio` 直接返回 0，于是 0% 只是推算值，界面却标成「服务端读数」，
误导用户，也让额度门禁在读数过期时按 0% 放行。

### Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| CF-QUOTA-R6 | **实时查询**：用应用已有的 ChatGPT 登录凭据主动查询 Codex 用量接口（先 `GET https://chatgpt.com/backend-api/wham/usage`，失败再试 `https://chatgpt.com/backend-api/codex/usage`），解析 5 小时与周两个窗口的 used_percent、reset，未知字段容错；只允许请求这两个地址，不跟随重定向 | 合成响应解析测试 + 只允许两个地址的常量断言 |
| CF-QUOTA-R7 | **何时查询**：`get_quota` 调用时、路由决策前（读数超过 2 分钟或已过重置点）、后台每 5 分钟刷新 | 代码接线 + 聚焦测试 |
| CF-QUOTA-R8 | **来源如实标注**：区分「实时查询」「回复头读数」「推算（已过重置点，待刷新）」；推算值不得标成服务端读数；实时查询失败时说明原因并退回回复头读数 | 组件测试（来源 + 读取时间 + 失败原因） |
| CF-QUOTA-R9 | **门禁以最新读数为准**：查不到实时读数时不得把「已过重置点」当成 0% 放行，应先尝试刷新 | 测试：过期读数仍判为越限；取得实时低用量后解除 |
| CF-QUOTA-R10 | **不泄漏凭据**：不得把令牌写进日志或审计；错误信息不含原始响应体 | 代码审查 + 错误为固定中文文案 |

### Implementation Notes

- 实时读数通过既有的 `codex_auth::valid_access_token()` 取得令牌，仅用于请求上述两个地址；
  错误映射为固定文案（「未登录或登录已过期」「网络连接失败」「用量接口不可用」「用量接口已变更」），
  不记录原始响应体，避免任何凭据或隐私内容进入日志与审计。
- 实时读数写入既有 `QuotaLedger`（同时按端点名与路由 base_url 记录），来源标记为 `LiveQuery`。
- 过期保护：`evaluate_cap` 中，非估算来源的窗口一旦 `resets_at <= now`，按 100% 处理（阻断），
  直到刷新到新的读数为止——这是本次返修对 CF-QUOTA-R3「重置后自动切回」的收窄：
  切回必须由实时读数确认，不能仅凭重置时间推断。
- 前端在 `SubscriptionQuotaSection` 展示来源、读取时间与实时查询失败原因。

本次仅修复实时额度读取与来源标注，不改变 R2 上限配置语义。

### CF-QUOTA-R2 后台入口补齐

- `get_quota` 返回与设置页相同的 `subscription_quota_status` 列表；不暴露凭据。
- `set_quota_cap` 使用端点键和至少一个 `five_hour_percent` / `weekly_percent`，
  取值为 1–100 整数；未提供的窗口保留原值，旧配置缺值默认 80%。
- 设置页与后台入口共用 `src/lib/subscriptionQuota.ts`，经设置 store 的 `save_settings`
  持久化后读取状态。只接受订阅端点；未知端点、非法参数和保存失败如实报错。
- 原有 socket 当前用户鉴权、权限与审计边界不变；修改上限记录端点和窗口值。
- R2 验收映射：`desktopDispatch.quota.test.ts` 覆盖后台→共享服务→真实设置 store；
  `headless_dispatch_quota.rs` 覆盖 public API、真实 socket 往返、1–100 参数域与审计；
  `SubscriptionQuotaSection.test.tsx` 保留设置保存断言；浏览器 light/dark 门禁验证设置回归。
- 本次仅补齐 R2，不修改 R1/R3/R4/R5 的计量、路由或布局语义。

- 计量窗口：Codex/ChatGPT 后端在响应头里给出 `x-codex-primary-*`（5 小时窗口）与
  `x-codex-secondary-*`（每周窗口）的已用百分比与重置倒计时；本实现按这两组头解析，
  并按 `window-minutes` 兜底映射（≤1440 分钟 → 5 小时窗口，否则每周）。
- 本地估算：按官方费率权重折算加权单位（输入 1x、缓存输入 0.25x、输出 4x），
  窗口预算默认取生产证据量级（5 小时 30M 加权单位、每周 250M），估算结果始终标记为
  `Estimated`，绝不冒充服务端读数。
- 到点自动换在**每个新回合的 route plan 构建时**生效：上一回合越过上限后，
  下一次 `resolve_route_plan` 会把该订阅端点排到回落端点之后；窗口重置后同一函数
  重新读到低于上限的用量，自动回到订阅端点。进行中的一轮不会重放副作用（复用既有
  replay latch 语义）。
- 上限只作用于订阅类端点（`ApiStyle::Chatgpt`），按量付费端点不受影响。
