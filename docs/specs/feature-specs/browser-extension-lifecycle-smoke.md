# E2E-008 L2：实现 `--browser-extension-lifecycle-smoke` 进程级冒烟

## 背景
- 2.0 门禁③：docs/testing/scenario-registry.json 的 E2E-008「浏览器扩展空闲、瞬断与多连接交错后同一会话自动续接」为 partially_implemented，remaining_gaps 第一条是「L2 --browser-extension-lifecycle-smoke 尚未实现」。已有 rust 单测（an_old_connection_closing_cannot_clear_the_new_extension 等）覆盖局部，缺一个启动正式二进制、走真实进程生命周期的冒烟。
- 动手前读：registry 中 E2E-008 定义、现有 `--history-session-smoke` / `--delivery-recovery-smoke` 的实现方式（照它们的结构做，不另起框架），docs/long-tasks/scenario-test-completion.md。

## 需求
- R1 在正式二进制上新增 `--browser-extension-lifecycle-smoke`：用确定性的本地 loopback 假扩展（不启动真实浏览器、不连外网），覆盖 registry 中的 fault_injection：扩展空闲、瞬断重连、新旧连接交错（旧连接关闭不能清掉新连接、旧代际上的命令被取消）、命令超时驱逐半开连接；断言同一会话自动续接、无人工 prompt、副作用不重复。
- R2 产出匿名 receipt，失败路径先写 receipt 再非零退出（照现有 smoke 约定）。
- R3 接入 CI：放进与现有 smoke 相同的 workflow/stage（PR 或 nightly，按 registry execution 字段），满足 validate_scenario_test_governance.py 的 hard-gate 要求。
- R4 本单**明确授权**修改 `docs/testing/scenario-registry.json` 的 E2E-008：automated_by 增加 `binary:--browser-extension-lifecycle-smoke`，从 remaining_gaps 移除已完成的 L2 条目，保留 L3/L4 条目；automation_status 仍为 partially_implemented。本地 validator 通过；scenario-gate-pr 的 cannot self-modify 由编排方处理。
- R5 先失败（未实现时 smoke 入口报错/目标失败），记录证据；在长任务记录追加进展。

## 约束
- 合成 fixture；不启动用户真实浏览器、不触碰用户 profile；不写生产 DB。

## Implementation Notes

- 需求与最低证据：R1 对应真实 WebSocket 生命周期集成测试及正式 executable receipt；R2 对应注入失败后退出码和匿名 receipt 的进程验收；R3/R4 对应合约测试、场景 validator 和 PR CI；R5 对应失败优先记录与长任务进展。
- Applicable Harnesses：Spec、Compatibility、Observation、Payload、AI Collaboration；不改 UI，不以 L2 证据替代 L3/L4。
- 会话为隔离内存中的合成 browser session，持续持有同一 bridge 和选中 tab；不声称覆盖生产数据库或真实桌面续接。
- 真机验收由编排方发版后完成：安装精确版本、在同一会话中选择 tab、触发扩展空闲/瞬断/重连/升级，核对自动续接、无人工 prompt、操作不重复与审计日志。
