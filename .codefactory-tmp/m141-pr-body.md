## Summary
M141：可信队列 `drain --watch` 对尚未完成的必需检查每 180 秒复查，最多 30 分钟；每轮仍执行 ruleset guard 和精确 head 检查。原单次 drain/dry-run 不变，不修改权限、strict 或 required checks。

Scenario-Test: CXD-002
README-Update: reviewed
README-Update-Reason: 仅修复内部队列调度可靠性，不改变公开产品、安装、隐私或平台承诺。

## Spec 与验收
规格：`docs/specs/feature-specs/m141-queue-label-trigger.md`，任务原文保留。
| Req | 结果 | 证据 |
| --- | --- | --- |
| R1 | PASS：核对真实运行，纠正 #811 手工触发归因 | 下方时间线 |
| R2 | 本地 PASS | `pnpm test:merge-queue` 74/74；M141-R2 新增测试先红（缺 watchQueue）后绿；运行中/缺失/旧 head 转绿、失败/hold/撤 arm/stop/dry-run、轮询预算 |
| R3 | 待可信默认分支启用后取证，不将单测算 live | 本 PR 由旧默认分支队列合并；合并后补实际 run/最后检查/合并时间线 |

## R1 现场记录（2026-10-21，Asia/Singapore）
| run id | event | conclusion | 开始–结束 | 观察 |
| --- | --- | --- | --- | --- |
| 39596784409 | pull_request_target | success | 20:04:58–20:05:16 | #811 agent-bridge-linux still running，单次 drain 退出 |
| 39596787462 | workflow_dispatch | cancelled | 20:04:59–20:05:14 | pending dispatch 取消，不是最后必需检查的完成事件 |
| 39596818160 | workflow_run | success | 20:05:13–20:05:39 | agent-bridge-linux still running |
| 39598832280 | workflow_run | success | 20:20:37–20:20:55 | scenario-gate-pr still running |
| 39598963613 | workflow_run | success | 20:21:37–20:22:13 | 自动合并 #811，merge 时间 20:22:04，6038e541a93da47aa059dcfeef6acd45c28d7389 |
| 39599026090 | workflow_dispatch | success | 20:22:06–20:22:27 | 比 merge 晚 2 秒，不能归因为手工救活 |

CI 39596773230 于 20:20:35 完成；精确 head 6e28d442fbef99dbbd12cfb537f3fe6e481d7c50 的 scenario-gate-pr 于 20:21:35 完成，29 秒后自动合并。任务所述“全绿 15 分钟”与 API/日志不符，实际尚在等检查。未找到本次最后 workflow_run 被取消的证据，不编造根因；有界复查覆盖早醒后立即退出窗口，减少对后续 webhook 的依赖。

## Harness / AI Collaboration
- Spec：原文与 R1–R3 映射。
- Compatibility：全部旧队列测试通过，单次 drain 与 M114 API 预算行为不变。
- Release：默认分支可信 checkout、权限、strict 与必需检查不变；不发桌面版本。
- Observation：每轮结构化 pass/guard/plan/outcomes 与真实运行记录。
- AI Collaboration：context scope 为任务、模板、队列与 GitHub API；假设 GitHub 正常调度；review point 为停止/门禁/预算边界；validation result 为队列 74 项和治理基线通过。按用户模板单执行流，未调用 delegate_tasks。
- 非 UI 任务，无浏览器或截图要求；未修改 Rust，不运行无关 Cargo 编译。

## 风险与回滚
待检查时 runner 最多占用约 30 分钟；180 秒间隔保护 API 预算，保持全局互斥。超出窗口仍由原 workflow_run/schedule 兜底，不承诺 GitHub 绝对调度 SLA。回滚只需撤销 workflow 的 `--watch`，无标签/持久状态迁移。不得绕过任何 scenario/trusted runner 门禁。
- 全量验证：`pnpm test` 156 files / 1044 tests PASS（含 lightModeAudit）；`pnpm build` PASS；统一 scenario-harness local PASS；scenario-test-governance --ci PASS；readme-contract PASS；governance baseline PASS；工作流 drain shell `bash -n` PASS。系统 Python 3.9 缺 tomllib，已切换已安装 Homebrew Python 后原门禁通过。
Spec Amendment：无需求变更；现场证据纠正事故归因。
