# Evolution Agent：Session 轨迹与持续改进闭环

## 1. Requirements Traceability

| Req ID | 来源 | 规范化需求 | Surface | 验证 | Owner |
| --- | --- | --- | --- | --- | --- |
| CF-EVO-R1 | 用户方案 | 持久 session 的真实工具生命周期进入规范化轨迹 | agent-loop + sqlite | Rust integration + real app DB assertion | development + QA |
| CF-EVO-R2 | 用户方案/安全 | 系统派生的工具参数、结果、错误、assistant/reasoning 入库前脱敏；normalized/Evidence 另行限长 | agent-loop + evidence | secret fixtures + DB/export grep | development + QA |
| CF-EVO-R3 | 用户方案 | project、Quick、task 三类持久 session 可分析；anonymous 零持久化 | chat + task + sqlite | route tests + real app | QA |
| CF-EVO-R4 | 当前缺陷 | 普通聊天无 task_run 时仍有 post-mortem 输入 | learning | unit + integration + real chat | development + QA |
| CF-EVO-R5 | 当前缺陷 | Evidence 读取真实 `tool_calls`，不查询不存在的表 | evidence | field-level pack assertion | development + QA |
| CF-EVO-R6 | 用户方案 | 失败率只统计真实执行 done/error，不把拒绝/legacy 当工具故障 | learning | detector/query tests | development + QA |
| CF-EVO-R7 | 用户方案 | 所有改进维持人工门禁，不自动合并/部署/发布 | learning + skills + settings | code review + UI path | planning + QA |
| CF-EVO-R8 | 仓库规则 | 真实 CodeFactoryDev 成功与边界路径验证 | desktop-ui | screenshot/video + DB evidence | QA |

## 2. Primary User Path

用户打开 CodeFactoryDev，选择真实项目和模型，完成包含工具调用的任务；系统将工具声明、权限结果、执行结果、耗时和脱敏摘要持久化。session 结束后，现有自进化审核界面能从这些记录产生或展示带证据的候选；用户决定接受、拒绝或启用更谨慎的工具门控。

## 3. Applicable Harnesses

- Spec Harness：Req ID、数据合同、状态与验收。
- Compatibility Harness：旧 SQLite、messages JSON 重放、增量 schema。
- Observation Harness：真实工具 route、状态、耗时、错误与 dropped 边界。
- Payload Harness：arguments/result/error 截断、脱敏、Evidence 导出。
- Viewport Harness：Profile 审核区在 1366×768 和窄窗口可操作。
- AI Collaboration Harness：规划、架构、QA 独立审查；明确当前实现与建议。

## 4. 测试矩阵

| 路径 | 场景 | 预期 | 最低证据 |
| --- | --- | --- | --- |
| Primary | allow 工具成功 | pending -> done，result/duration 可追溯 | DB row + 工具卡 + screenshot |
| Failure | 工具返回错误 | pending -> error，error 脱敏 | DB row + 实际输出 |
| Permission | ask 后拒绝 | status=denied、duration=0，不计工具失败 | UI decision + DB row |
| Hook | pre-tool cancel | status=denied，不执行工具 | hook log + DB row |
| Runtime error | dispatch 返回 Err | status=error 后再传播错误 | regression test + DB row |
| Chat | 无 task_run 的普通/Quick 会话 | 生成有限脱敏 session summary | prompt-builder test + real chat |
| Privacy | anonymous 同类调用 | DB/session/learning/evidence/cost 计数不变 | 前后计数 |
| Privacy | user 输入含测试 secret，模型/工具复述 | 用户原始消息按既有历史保留；assistant/tool/trace/Evidence 均不复制原值 | DB 字段级 grep |
| Compatibility | v1.43.0 旧 DB | 启动后表/索引存在，旧消息可重放 | migration fixture |
| Evidence | 生成 evidence pack | 读取 normalized rows，含 status/error/duration，无 secret | pack field assertion |
| Analysis | 运行跨会话挖掘 | 只基于 done/error，真实信号非 fixture | query + UI + DB |

## 5. 完成边界

单元测试、构建、UI 空态或一条非空数组都不是完成。Phase 0 仅在真实 app 主路径、边界路径、持久化、匿名、脱敏和 Evidence 全部有证据后完成。完整 Evolution Agent 仍需候选状态机、统一 Review 工作台、materializer 与通用 Evals；这些在后续 Req 扩展中交付。
