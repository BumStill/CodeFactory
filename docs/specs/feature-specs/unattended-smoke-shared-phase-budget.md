# Unattended smoke 预算建议：本 PR 已撤回实现

## 状态

本文件保留为审查记录，不代表已实现规格。PR #962 不修改 unattended smoke driver 或 agent-core 预算断言。

## 失败原因与证据

head a067ddd2373c56d86c301bd6ecdda80a6f712cb0 的 execute-windows 在 2.6 秒内退出 1。runner-receipt.json 包含所有目标失败和 runner_execution_failed，尚未执行目标命令。

原因是 src-tauri/src/agent/unattended_smoke.rs 位于 tools/governance/scenario_case_execution.py 的 DRIVER_FILES 中。可信 main planner 绑定 driver_sha256，candidate 修改该 driver 后，validate_case_execution_inputs 返回 case canonical driver digest does not match the trusted plan。execute_plan 在任何目标启动前阻止执行，且当前入口仅将拒绝写入收据，不打印原因。

## 本地复现与修复

- 默认 --policy-repo . 把 candidate 当作自身策略源，不能证明 CI 的跨树可信边界。
- 从 origin/main 导出独立策略目录后运行标准 run_scenario_harness_gate.py --stage local --repo . --policy-repo <main策略目录> --base-ref origin/main，修复前明确报 trusted scenario gate root cannot self-modify ... unattended_smoke.rs。
- 撤回 driver 修改及配套预算断言后，同一入口通过；逐文件 driver 篡改拒绝测试继续通过。
- 不改执行器、validator、可信根列表或摘要校验，不绕过门禁。

## 后续建议

phase-two/three/four 的 45s 等待预算与更清晰的 preflight 拒绝日志，需要独立的外部治理变更任务，不能在普通发布重试 PR 中自我修改。

## 追溯与验证

| 项目 | 结果 |
|---|---|
| R1–R5 发布基础设施重试 | 保留本任务范围 |
| CF-USB-1/2/3 预算建议 | 撤回实现，独立 follow-up |
| 标准本地 gate + 独立 main policy | 修复前失败，撤回后通过 |
| test_preflight_checks_actual_platform_head_and_every_protected_driver | 通过，不削弱可信边界 |

Applicable Harnesses: Spec、Observation、Release、AI Collaboration。真实 CI 需新 head 的 execute-windows、receipt、scenario-gate-pr 全部成功，才恢复合并队列。
