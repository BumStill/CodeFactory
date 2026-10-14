# M77b / M85 真机返修规格

## 批准任务（原文）
任务：CI 全绿后交付契约校验失败导致不打合并标签（M77 复发）+ 回落链失败原因误导（M85）

真机证据（v1.88.1，2026-10-14）：
- M77 复发：会话 94c0455d 的 #653，CI 全绿，PR 状态 CLEAN。deliver_changes 返回 `delivery_contract_invalid: invalid delivery outcome: state=delivered, recoverable=false, recovery_class=None`。delivery_runs 停在 `complete | awaiting_completion_arbitration | ci_green`，没有打 `merge-queue: armed`；objective 为 `waiting_system / external_state_uncertain`。最后由编排方手动打标签。#651 修的是「CI 全绿后打标签」这条路径，但交付结果在契约校验这一步先失败了，根本没走到打标签。
- M85：主模型 chatgpt 偶发 TRANSPORT_UNREACHABLE；deepseek 不在可用时段，被正确跳过；openrouter 报 AUTH_MISSING。objective 的失败说明只写了 `AUTH_MISSING: openrouter 尚未配置凭据`，看不出真正原因是主模型临时连不上。约 2 分钟后重试 GPT 成功，但编排方已经被这条说明带偏。

要做的：
1. M77b：用合成数据复现「ceiling=merged，CI 全绿，交付结果 state=delivered」的时序，找出契约校验为什么拒绝这个合法结果（`recovery_class=None` 时 `recoverable=false` 的组合是否被误判），修复后应继续执行打标签。断言：CI 全绿之后一定会打 armed，objective 不会进入 external_state_uncertain。
2. M85：回落链全部失败时，失败说明要按链列出每一环的原因，并以主模型的可恢复错误为主。例如：「GPT 临时连不上，正在重试；DeepSeek 高峰时段跳过，12:00 可用；OpenRouter 未配置」。会话里展示的说明与 objective 事件里记的保持一致。
3. 每一项都先写复现测试，再修。

约束：不改 `.github/` 工作流、场景注册表与校验器；只用合成数据；其余同模板。

## Requirements Traceability
| Req ID | 要求 | 最低证据 |
| --- | --- | --- |
| CF-M77B-R1 | 合法 delivered 降级结果通过契约，CI 绿后继续 armed；不进入 external_state_uncertain | 合成 delivery 结果/工具投影与队列集成回归 |
| CF-M85-R1 | 所有失败按链显示，含时段跳过与下次可用时间，主模型可恢复错误主导恢复分类 | 合成混合失败链回归与 transport 集成 |
| CF-M85-R2 | 会话与 objective 使用同一失败说明 | transport 返回值及 objective 持久化路径验证 |

## Applicable Harnesses
Spec Harness、Compatibility Harness、Observation Harness、Release Harness（仅 PR/CI/队列交付，不发版）、AI Collaboration Harness。无布局变更，不启用 Viewport Harness。

## 测试矩阵与验收分工
- delivered / lowered ceiling 合法，真正矛盾的 recovery 字段仍 fail closed。
- CI 绿后的 armed，失败重试与停止边界维持既有行为。
- GPT transport failure + DeepSeek 使用时段跳过 + OpenRouter 缺凭据：三环原因可读、按配置顺序、主错误可恢复。
- 单端点凭据错误不得变成 transport outage；成功回落不变。
- 真机验收由编排方发版后完成：安装新版本，复走 merged 交付检查 armed 与 objective；再模拟首选临时不可达、时段跳过、末端缺凭据，核对会话说明与 objective 审计事件一致，恢复首选后确认同一目标续接。

## Implementation Notes
- 所有 fixture 均为合成数据，不访问生产 SQLite 或真实模型凭据。
- 使用当前 managed worktree/branch；不创建另一个分支。
