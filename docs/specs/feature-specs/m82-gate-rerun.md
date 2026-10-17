# M82：失败任务重跑的回执识别

## 已批准任务（原文）

# 任务：只重跑失败的任务后，场景门禁也要认回执（M82）

按 `/Users/leo/.codefactory/agent-tools/task-template.md` 执行（harness + SDD + TDD + 自验收 + §8 收尾；交付 ceiling: merged）。
**信任根授权**：用户 2026-10-17 明确批准本任务修改场景执行和门禁的回执逻辑（`tools/governance/scenario_execution.py`、相关 workflow），以及必要的注册表条目。候选 PR 不能自证：如果 `scenario-gate-pr` 因为 cannot self-modify 失败，属于预期，由编排方按治理流程处理，**不要**绕过门禁。其它检查必须全绿。

## 真机证据
- 10-13 #649、10-16 #658：Windows 无头验收偶发失败后，用 `gh run rerun --failed` 只重跑失败的任务，结果为通过；但 scenario-gate-pr 重跑后仍然报 `missing successful execution receipt for pnpm:test:…headless` 和 `scenario execution receipt contains invalid runner artifacts`。只能推一个空提交，让整套检查在新 head 上从头跑，每次多花约 25 分钟。

## 要做的
1. 回执的收集要支持同一 run 的多个 attempt：同一个 head SHA、同一个 run 中，某个 target 在任意一个 attempt 里成功，就视为成功。产物按 attempt 收集，「invalid runner artifacts」的判定不能因为 attempt 序号不同而误判。
2. 安全边界不变：只认同一个 head SHA 的回执；必须仍由 main 上可信的 runner 判定；失败过的 target 必须有一次真实的成功 attempt，不能靠「没有记录」蒙混过去。
3. 先用合成的 run / attempt 数据写失败测试（覆盖：第一次 attempt 失败、第二次 attempt 成功；回执只出现在 attempt 2），再修。

## 约束
只改上面列出的文件和对应的测试或注册表条目；只用合成数据；其余同模板。

## 标题
`fix(scenario-gate): a target that passes on a re-run attempt counts, so a flaky job no longer needs a full fresh run`

## Implementation Notes

| Req ID | 已批准要求 | 最低证据 |
| --- | --- | --- |
| M82-R1 | 同 run / head 的跨 attempt 成功可累计；attempt 2 独有回执可用 | 合成回执与 GitHub API 测试 |
| M82-R2 | 保持 base/head、runner、digest、case 校验；失败或缺失不可冒充成功 | 负向测试与现有安全测试 |
| M82-R3 | 保持 main 可信 runner 和不自证限制 | workflow 保留 trusted base checkout；本地门禁及 CI |

Applicable Harnesses：Spec Harness、Compatibility Harness（旧单 attempt 回执）、AI Collaboration Harness、Observation Harness（回执字段断言）。不修改 GUI 或 Rust；真实安装版验收由编排方发版后完成，核对失败重跑后同 head 门禁可认回执、未成功 target 仍阻断。

技术约束：artifact 名称携带 run attempt，旧回执名称兼容；同 runner 同 attempt 重复仍拒绝。跨 attempt 只挑选通过验证的 target/case 成功证据，错误身份或畸形字段仍 fail closed。来自不同 run 的产物不混用（GitHub 下载范围限定 run）。
