# 任务失败归因与修复闭环规格

## 范围

本规格定义 CodeFactory 主 Workspace 任务系统中的失败归因与修复闭环。目标是让用户看到“为什么失败、系统正在如何修复、何时才确实需要核心输入或业务决定”，并让每次修复都回到真实项目任务路径。

Terminal-Bench 2.1 只能作为失败样本和能力评估输入，不允许把分类、提示或修复逻辑写成 benchmark 专用分支。

objective ownership、完成语义与用户回交以 `objective-recovery-control-plane.md` 为准。本规格中的 `repairable` 只决定当前 approach，不能把技术失败变成人工重试终态。

## Requirements Traceability

| Req ID | User request | Normalized requirement | Surfaces | Validation method | Owner |
| --- | --- | --- | --- | --- | --- |
| CF-FR-R1 | 能力提升要落到 CodeFactory 主产品路径 | Workspace 任务列表对 failed/cancelled task 展示失败类型、证据来源和下一步修复建议 | backend + UI | Rust classifier tests + Workspace UI test + real app task-column check | development |
| CF-FR-R2 | 形成迭代 loop | system-owned 失败必须形成“归因 -> 持久 remediation -> 修复/退避/对账 -> 重新执行 -> 验证证据”的自动闭环；`repairable=false` 只禁止当前策略盲重跑，不能停止 objective 或生成技术重试 CTA | task scheduler + objective supervisor + UI | exhaustion/remediation/restart tests + product path verification | development |
| CF-FR-R3 | 不针对 bench 定制 | 分类规则只读取通用 task 字段：`status`、`error`、`result`、`verification_results`；不得读取 benchmark task name 或 Terminal-Bench artifact path | backend | unit test + code review grep | QA |
| CF-FR-R4 | 用户能区分责任边界 | provider/credential、permission timeout/explicit deny、shell runtime、test failure、verification failure、cancelled、unknown 必须分开展示，并同时标明 system/core-input/business-decision/user-cancel owner | backend + UI | fixture classification + decision-router tests | QA |
| CF-FR-R5 | 兼容旧数据 | 不要求 SQLite schema migration；旧 `task_runs` 读取后按现有字段派生 attribution | storage + Tauri serialization | cargo test + existing app DB smoke | development |
| CF-FR-R6 | 用户回交必须必要且一次性 | 只有穷尽安全替代后的 `core_input_required` 或无安全默认的不可逆 `needs_business_decision` 可显示一次结构化动作；输入/选择满足后同一 objective、session 与 checkpoint 自动续接，不注入伪用户消息，不要求再触发重试 | Workspace + Settings + objective supervisor | typed decision + auto-resume + forbidden-copy tests + real App | development + QA |

## Primary User Path

P-FR-1: 用户打开 CodeFactory，进入一个 project session。左侧任务列显示任务状态。如果某个任务发生技术失败，系统在任务行下方显示失败归因、recovery owner、当前 remediation 阶段、最近真实进展和下一次观察时间。scheduler 自动从同一 session/objective 的持久 checkpoint 重新分派；当前 approach 不安全或预算耗尽时转入 durable remediation，而不是保持 failed 等待用户推动。任务恢复后，状态、attempt journal、验证结果和 evidence pack 继续更新。

P-FR-2: 如果失败来自 provider/credential、permission channel 或 shell runtime，UI 必须让用户看到这不是“模型不会写代码”的同类问题，同时显示系统正在退避、换 route、重建授权通道或修复运行环境。只有确实缺少不可替代凭据/额度/授权输入时才显示合并后的 core-input 卡；输入满足事件到达后系统自动续接。显式拒绝或取消绑定 action signature，系统不得换工具绕过。

## Applicable Harnesses

- Spec Harness: 本规格、Req ID、测试矩阵和证据要求必须随代码提交。
- Compatibility Harness: 不破坏旧 `task_runs` schema、旧 session 和已有任务列表。
- Observation Harness: 失败归因必须来自可审计字段，不得只显示模糊文案。
- AI Collaboration Harness: 归因规则必须标明假设和验证结果；bench 结果只能作为样本，不能成为专用逻辑。
- Release Harness: 发布后必须在安装版 CodeFactory 主路径验证设置、模型选择器、任务修复入口和归因展示。

## Failure Taxonomy

| Kind | 中文标签 | 典型证据 | 默认下一步 |
| --- | --- | --- | --- |
| `model-provider` | 模型/Provider | HTTP 402/429/5xx、insufficient balance、invalid API key、unauthorized、rate limit | 系统先退避/刷新/换兼容 route；必要凭据或额度才聚合为 core input |
| `permission` | 权限/策略 | timeout、channel close、policy deny、hard deny、用户明确拒绝 | timeout/channel close 由系统重建；policy/hard deny 换安全策略；明确拒绝绑定并停止等价副作用 |
| `shell-runtime` | 运行环境 | command not found、No such file、executable unavailable、spawn/ENOENT | 系统诊断 PATH/依赖/命令环境并以新 approach 重派 |
| `test-failure` | 测试失败 | npm test/pytest/cargo test failed、assertion、expected/actual | 系统基于失败断言修改实现并重跑最小测试 |
| `verification` | 验收失败 | `verification_results` 存在 failed check，或 summary 表示 final verification failed | 系统读取失败验收项、修复并重跑同一检查 |
| `cancelled` | 已取消 | 显式用户 cancel/deny 对应的 status | 终止绑定 objective/action；不以其它工具绕过 |
| `unknown` | 未分类 | 没有足够字段 | 系统诊断并进入 fail-closed remediation，不猜用户责任 |

## Testing Matrix

| Scenario | Expected evidence |
| --- | --- |
| failed task with failed `verification_results` | classified as `verification`, UI shows `验收失败` and next action |
| provider billing/credential error | classified as `model-provider`, marked not blindly repairable |
| missing command or executable | classified as `shell-runtime` |
| automatic repair loop with mixed failed tasks | repairable approaches are retried; exhausted/provider/runtime/timeout rows enter durable remediation and same-session redispatch, not manual failed state |
| six provider credential failures sharing one missing input | one session/objective core-input request aggregates the six rows; request count stays one; completion event auto-resumes all eligible work |
| explicit user deny on one action | only the matching action signature is cancelled/denied; equivalent side effect is not retried or disguised |
| unknown technical blocker | fail-closed diagnosing/remediation state with system owner; no prefilled chat message or technical CTA |
| paused pending tasks without failures | supervisor owns and dispatches remaining tasks; task rows show next observation without a continue button |
| pending tasks plus failed technical task | one objective projection shows queued/running remediation and preserves attempt history |
| assertion/test failure | classified as `test-failure` |
| cancelled task | classified as `cancelled` |
| old task without new persisted fields | list still loads and derives attribution from existing fields |

## Evidence Pack Requirements

每次交付至少记录：

- classifier unit test results。
- Workspace UI test result（system-owned 无 CTA、core input 一次请求并自动续接、显式拒绝不绕过三条动作链）。
- `pnpm build` 和治理基线结果。
- 真实安装版或 dev app 主路径截图/观察：任务列中失败归因可见，临时验证数据已清理。
