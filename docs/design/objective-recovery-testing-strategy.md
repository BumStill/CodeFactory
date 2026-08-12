# Objective Recovery Control Plane：测试策略

## 目标

证明 `CF-ORC-R1..R32` 在正式产品端到端成立，而不是证明某个模块存在重试代码。每个场景都必须同时断言状态真相、用户投影、恢复行为、副作用幂等和跨进程连续性。

## 测试金字塔

| 层级 | 比例与职责 | 关键范围 |
| --- | --- | --- |
| Unit/contract | 多、快速 | DecisionEnvelope、router、arbiter、DB constraints、failure classifier、forbidden copy |
| Integration | 中等 | SQLite migration/claim race、AgentLoop outcome、permission/provider/task/auth adapter、receipt fence |
| E2E/fault injection | 少、强证据 | Tauri/CodeFactoryDev 跨进程 kill/restart、真实 UI、browser、GitHub canary、release artifact |
| Production review | 发布后 | 24h 聚合 KPI、ownerless/reprompt/false-complete/duplicate evidence |

## Failure-first 顺序

1. DB 拒绝非法 `system_owned + blocked/completed/requires_user_action`；
2. AgentLoop 返回 typed outcome，desktop 不丢弃；
3. permission timeout/channel closed 进入 waiting_system，显式 deny 保持终止；
4. stream Done 后 system-owned objective 卡仍可见；
5. scheduler attempt exhaustion 自动进入 durable remediation；
6. auth 成功后自动续接且不增加 user message；
7. browser pairing/2FA lease 保活与自动 attach；
8. release branch policy 自动使用支持的 merge/queue 路径；
9. 跨进程 fault matrix；
10. KPI/semantic release gate。

每项先在未实现代码上观察红测，再实现并保存 before/after 命令结果。

## 核心断言模板

所有 fault case 共同断言：

- `objective_id` 和 requested acceptance 不变；
- 没有新增伪 user message 或技术 CTA；
- `turn_settled_at/stream_closed_at` 可写，但 `objective_completed_at` 仅由 arbiter 写；
- system-owned 状态始终有 owner、lease/remediation 和 next safe observation；
- receipt/counter 证明外部副作用至多一次；
- 恢复后使用同一 canonical PR/head/session/task/browser identity；
- 明确 deny/cancel 不被绕过；
- UI 与 DB typed state 一致。

## Fault Matrix

| Domain | Inject | Success path | Boundary path |
| --- | --- | --- | --- |
| Permission | fake clock timeout、channel drop | rebuild wait/Allow 后自动 resume | explicit deny remains terminal |
| Provider | 429、503、connection reset | zero-output route/backoff success | partial output prevents blind replay |
| Auth | 401 + refresh fail | reauth event resumes checkpoint | prior side effect forces reconcile |
| Context | overflow + process kill | durable snapshot/compaction resume | oversized core input becomes one request/incident |
| Task | repeated test/verification fail | durable backoff/new approach/same session | explicit cancel stops only intended task/objective |
| Browser | extension unpaired、2FA wait | managed fallback or lease-preserving attach | submit result unknown -> reconcile |
| Delivery | CI failure、rules 403、branch mismatch、remote timeout | same PR/head repair/reconcile | unknown mutation never replayed blindly |
| Release | checks green + protected merge、workflow cancel | auto/queue merge and same batch resume | hold remains structured business decision |
| Process | SIGKILL at each wait state | claim within 30s and resume | ambiguous legacy row -> legacy_orphan |
| Update | update ready with live owner | waits safe point, restarts and resumes | unknown owner fails closed without losing objective |

## Frontend 与 Viewport

- reducer/component：system-owned、core input、business decision、completed/cancelled、legacy orphan；
- hydration：streaming=false 仍展示 active objective；无 assistant row 时绑定 root user turn；
- negative copy assertions：生产 UI 不包含技术性 retry/continue/resend CTA；
- 1366×768、800×600、390×844 截图；键盘焦点、ARIA、motion-reduce；
- Task Workspace 正式组件必须读取 resume/remediation snapshot。

## Release 与正式版证据

1. focused Rust/TS/Python tests；
2. full Rust workspace、frontend、lint/typecheck/build、governance；
3. CodeFactoryDev 成功路径与每个 fault domain 的边界路径；
4. PR checks、merge、release workflow、artifact digest/signature/install；
5. 正式安装版精确 version/commit，重复执行 fault smoke；
6. 24h read-only production gate 与 KPI 聚合；
7. 零容忍指标任一非零则结论保持“不满足”。

## 覆盖目标

- DecisionRouter/CompletionArbiter/DB transitions：100% decision variant/illegal transition；
- 每个 domain adapter：至少一条自动恢复、一条 core-input 或 safety boundary、一条 restart；
- 所有外部 mutation adapter：receipt/counter 幂等测试；
- UI typed state：全部状态 + 三个目标 viewport；
- migration：fresh、v1.79.1、identity-incomplete legacy 三类 fixture。

## 不可接受的替代证据

- mock-only、HTTP 200、非空数组、日志出现 supervisor、heartbeat 数量；
- unit test 通过但真实 App 未走主路径；
- PR/CI/merge 或 release metadata 代替安装版行为；
- 用户手工点击“继续/重试”后才成功；
- 隐藏错误、降低 requested acceptance 或删除失败记录后得到的零指标。
