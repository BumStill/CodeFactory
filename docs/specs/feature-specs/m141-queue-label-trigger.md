# M141：打上 merge-queue: armed 后队列仍不立即运行（M126 后仍需健康检查补触发）

## 证据
- 10-21 #811：CI 全绿且 armed 状态 CLEAN 超过 15 分钟未合并，健康检查 20:22 手动 `gh workflow run merge-queue.yml` 后才合并；#768（M126 前）同类。今日健康检查共补触发 ≥3 次（#783、#811 等）。
- merge-queue.yml 已有 `pull_request_target: [labeled, unlabeled]`、`schedule: */10`、`workflow_run`。
## 需求
- R1 查明为何没被 labeled 事件或 workflow_run 合并：例如标签由 GITHUB_TOKEN/交付工具在 CI 完成前打上，labeled 那次运行时检查未完成→跳过；随后 CI 完成的 workflow_run 运行被 concurrency 挂起后取消（日志里多次见 `workflow_run cancelled`）；schedule 在 GitHub 上延迟/丢失。把真实运行记录（run id、事件、结论、时间）列在 PR。
- R2 修复：保证「最后一个必需检查完成」之后必有一次队列运行能看到最新状态（例如 concurrency 不取消排队中的运行、或队列运行结束前若有新事件则再跑一轮、或交付工具在 CI 全绿时 dispatch 队列）。不削弱任何门禁。
- R3 验证：PR 写明修复后一次真实「打标→CI 完成→自动合并」的时间线（≤5 分钟内开始合并）。
## 约束
- 改工作流属 release-facing，写明风险与回滚。授权交付（ceiling: merged）。不要超出规格自加范围。

## Implementation Notes

- 现场纠正：#811 的 CI 于 20:20:35 完成，scenario-gate-pr 于 20:21:35 完成；workflow_run 39598963613 于 20:21:37 启动并在 20:22:04 合并。手工 dispatch 39599026090 于 20:22:06 才启动，不能归因为手工救活。原始任务文字保留于上文，不更改验收语义。
- labeled 39596784409（20:04:58–20:05:16，success）和 workflow_run 39596818160（20:05:13–20:05:39，success）都因 agent-bridge-linux running 退出；workflow_run 39598832280（20:20:37–20:20:55，success）因 scenario-gate-pr running 退出。dispatch 39596787462（20:04:59–20:05:14）cancelled，不能据此断言 CI 完成事件被取消。以上时间均为 Asia/Singapore。
- 实现：工作流使用 `drain --watch`，在必需检查缺失、过期、运行中时每 180 秒重读可信队列，最多 11 次（30 分钟），每次仍执行 ruleset guard 和精确 head 检查。无 armed、hold、失败、dry-run 或 stop 不轮询。workflow_run/schedule 原触发器保留；不修改权限或门禁。
- 风险：有待完成检查时 runner 最多占用约 30 分钟；全局互斥保持，pending 事件可合并但当前运行会持续复查。超出窗口由原 workflow_run/schedule 兜底，不宣称 GitHub 调度有绝对时限保证。
- 回滚：撤销 `--watch` 参数即可恢复原单次 drain；不改变标签与持久状态。

## Requirements Traceability / 测试矩阵

| Req ID | 验收 | 最低证据 |
| --- | --- | --- |
| R1 | 核对真实触发与合并因果 | PR 记录 run id、事件、结论、当地时间与日志 |
| R2 | 无后续事件时检查转绿仍自动合并；失败/hold/撤 arm/stop 不绕过 | M141 合成适配器行为测试先红后绿；现有队列全部回归 |
| R3 | 真实打标、最后检查、自动合并时间线 | 合入可信默认分支后真实队列运行证据，PR 更新自验表 |

## Applicable Harnesses

Spec：逐字任务与 R1–R3 映射；Compatibility：原 drain 单次调用不变；Release：可信 ref/权限/strict guard 不变与回滚；Observation：每轮结构化日志及真实 run；AI Collaboration：仅本任务文件、队列实现与 GitHub 记录为 context scope，假设 GitHub 仍可调度默认分支事件，review point 为停止/门禁/预算边界，validation result 记录于 PR。本任务模板禁止 delegate_tasks，使用单执行流。
