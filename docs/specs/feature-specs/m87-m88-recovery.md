# M87 / M88 / M89：取消如实与睡眠后的持续恢复

## 批准任务原文

按 `/Users/leo/.codefactory/agent-tools/task-template.md` 执行（harness + SDD + TDD + 自验收 + §8 收尾；交付 ceiling: merged）。
相关规格：`docs/specs/feature-specs/truthful-delivery-state.md`（CF-TRUTH-R3：explicit_cancel 只能来自真实的停止请求）、`docs/specs/feature-specs/model-usage-windows.md`（CF-WINDOW-R5：全部不可用时等待，到点自动继续）。

## 要做的（逐字保留）
1. **M87**：找出是谁把内部的 `cancelled` 错误（例如某个 future 被取消、被抢占或超时）当成了用户取消。用合成时序复现，断言没有取消意图记录时 provenance 不得为 explicit_cancel，objective 应进入可恢复状态，并且 send 能开启新一轮。修复后，这类会话要能被续上。
2. **M88**：凡是 objective 处于等待（active 或 waiting_system），都必须有下一次观测时间，并且到点真的重试。回落链里任何一环的可用性发生变化（时段切换、额度重置、网络恢复），都要尽快触发重试；最晚不超过有界的退避上限，例如 5 分钟。用注入时钟的集成测试复现「14:25 全断 → 18:00 闲时 → 自动继续」。
3. 每一项先写失败测试，再修。

## 补充要求（逐字保留）

新归属方要能对账旧的 in_flight 请求（例如确认它已经不在进行，或已经结束，就把它结算成可恢复的失败），然后继续执行。复现测试必须覆盖这个时序。

1. 存在 active / waiting_system objective 时，应用持有 macOS「阻止空闲睡眠」电源断言（IOPMAssertion PreventUserIdleSystemSleep，或者等价的 NSProcessInfo beginActivity）；没有任务时释放。Windows 上用对应的 SetThreadExecutionState。
2. 睡眠或时钟跳变之后，租约过期不能当作「换了持有者」：同一个进程实例要能续约，或者对账自己的 in_flight 请求，然后继续。复现测试要用注入时钟模拟「睡 15 分钟后醒来」。

## Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| CF-TRUTH-R3 / M87 | 无持久停止意图不得 explicit_cancel；可继续 | 合成 SQLite + outcome + send 回归；真实停止对照 |
| CF-WINDOW-R5 / M88 | 等待有下一次观察，到点重新选择可用路由 | 注入时钟覆盖 14:25 到 18:00、网络与额度变化；观察不超过五分钟 |
| M88-OWNER | 同 revision 接管对账无副作用的旧 in_flight | 旧 owner 围栏、新 owner 成功 admission；副作用反例 |
| M89-POWER | 仅有 active / waiting_system 时持有平台防空闲睡眠 | 合成 objective 驱动 acquire/release；平台 API 与本机断言 |
| M89-LEASE | 同 owner/epoch 过期可续约；真实接管不能续旧租约 | 注入 15 分钟时间跳变与 epoch 对照 |
| M88-TRANSPORT | 「连不上」等待到期必须被重新观测：路径恢复（或超窗）即转成可认领的续跑，仍不通则重排下一次观测，不扣恢复次数 | 合成 SQLite + 本地可达/不可达端口：到期后可认领；仍不通时下次观测在未来且无排队尝试 |

## Applicable Harnesses

Spec、Compatibility（取消/授权/副作用围栏）、Observation（持久失败原因和下一次观测）、Release（仅合并，not live）、AI Collaboration（单执行流，模板禁止 delegate_tasks）。无 UI 布局变更则不启用 Viewport。

## 约束与验收分工

不改 `.github/` 工作流、场景注册表与校验器；只用合成数据。仅在当前 managed 分支修改。CI 全绿后交给 merge-queue，禁止手工合并。真机验收由编排方发版后完成：核对无停止意图的恢复、等待路由重新请求、pmset 防空闲睡眠断言、无任务释放及睡眠后 lease/request 恢复；本会话不改生产数据库。

## Implementation Notes / 当前证据

- M87 新回归 `m87_missing_stop_authority_is_not_user_cancel` 先失败后通过：停止意图读取失败旧实现返回 true，修正为无正向证据不归因用户停止。尚须完整取消/继续时序覆盖。
- M88 新回归 `m88_same_revision_takeover_reconciles_old_in_flight` 先失败（真实错误 `previous provider attempt "in_flight" cannot continue under this live owner`）后通过。新 durable owner/epoch 在 begin_attempt 的同一写事务中只结算无副作用、无未决 receipt 的旧请求，旧 owner 仍 fenced。
- M88-TRANSPORT（编排方 2026-10-16 补充，真机根因）：`bound_transport_outage` 在路径仍不通时给 objective 铸造 remediation_id 却**不插入** remediation 行（避免空耗模型回合），而认领循环只认领真实存在的行——全代码库没有任何地方再去观测这种等待。真机证据：会话 7460b39a 10-15 12:59 一次网络抖动后，objective 指向不存在的 remediation 813fc447，停在 waiting_system 数小时。修复：`ObjectiveStore::reobserve_transport_probe_waits` 在每次 supervisor 轮询认领前运行，对到期的此类等待按同一探测与时间窗策略决定——仍不通则把下次观测推后一个探测间隔（不超过五分钟上限）；路径恢复或超窗则把已铸造的 remediation 落成 queued 续跑，交给原有认领路径（超窗后续跑再失败时按原逻辑如实结算失败）。回归：`m88_due_transport_wait_resumes_once_the_path_answers`（先因方法不存在无法编译，后通过）、`m88_due_transport_wait_still_down_is_rescheduled_not_dropped`。
- M89：`power_assertion` 在存在 active / waiting_system objective 时持有 PreventUserIdleSystemSleep（Windows：SetThreadExecutionState），否则释放；同 owner/epoch 在睡眠导致的租约过期后可续约（`m89_same_owner_renews_after_fifteen_minute_clock_jump`）。注意：合盖或用户主动睡眠不受空闲断言约束。
