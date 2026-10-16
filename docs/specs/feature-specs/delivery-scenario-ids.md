# M95：交付工具自己计算 Scenario-Test

## 原始任务（原文）
交付工具自己算 Scenario-Test，不再让 agent 手写（M95）。

1. deliver_changes 在创建或更新 PR 之前，自己运行 tools/governance/validate_scenario_test_governance.py（只调用，不修改校验器），用当前分支相对 base 的改动算出受影响的场景 ID：agent 没写时自动填上；写了但不一致（未知 ID、缺 ID、重复行）时，以校验器结果为准自动改正，并在交付结果里说明改了什么。README-Update 的两行仍由 agent 提供，缺了照旧拒绝。
2. 校验器本身无法运行时（例如环境缺依赖），如实报错，不得放行。
3. 先写失败测试（用合成仓库和合成正文，覆盖 none / 缺失 / 未知 ID / 重复行四种情况），再修。

约束：不改 .github/ 工作流、场景注册表与校验器；只用合成数据。

## Requirements Traceability
| Req ID | 要求 | 最低证据 |
| --- | --- | --- |
| M95-R1 | 创建、更新、合并前收敛正文，自动计算唯一 Scenario-Test 行，报告纠正内容 | 合成仓库覆盖 none、缺失、未知、重复、漏 ID 和幂等 |
| M95-R2 | 校验器、解释器、base 或注册表不可用时 fail closed | 子进程非零与原始错误断言 |
| M95-R3 | README 两行由调用者提供，缺失仍拒绝 | 预检回归测试 |

## Applicable Harnesses
Spec、Compatibility（旧正文与恢复）、Observation（纠正说明与错误）、AI Collaboration（独立审查）。无 UI 变化；不启用 Viewport。此次仅合并，不发版。

## 测试矩阵与实现说明
Python 适配器直接调用未修改校验器的加载、git diff、版本文件排除、场景影响计算及正文契约 API；不另写路径映射规则。Rust 通过受控开发命令运行适配器，所有 PR 正文远端变更前都执行。测试使用临时 git 仓库及合成注册表，调用真实校验器。

真机验收由编排方发版后完成：交付正文分别传入 none、缺失、SC-001、重复行，核对远端正文只有一条精确声明、交付结果列出修正；隐藏校验器后确认没有 PR 变更；缺 README 字段确认拒绝。
