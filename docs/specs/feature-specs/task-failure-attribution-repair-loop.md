# 任务失败归因与修复闭环规格

## 范围

本规格定义 CodeFactory 主 Workspace 任务系统中的失败归因与修复闭环。目标是让用户看到“为什么失败、下一步该怎么修、是否适合自动重试”，并让每次修复都回到真实项目任务路径。

Terminal-Bench 2.1 只能作为失败样本和能力评估输入，不允许把分类、提示或修复逻辑写成 benchmark 专用分支。

## Requirements Traceability

| Req ID | User request | Normalized requirement | Surfaces | Validation method | Owner |
| --- | --- | --- | --- | --- | --- |
| CF-FR-R1 | 能力提升要落到 CodeFactory 主产品路径 | Workspace 任务列表对 failed/cancelled task 展示失败类型、证据来源和下一步修复建议 | backend + UI | Rust classifier tests + Workspace UI test + real app task-column check | development |
| CF-FR-R2 | 形成迭代 loop | `修复失败项` 不只是按钮；它必须基于失败状态形成“归因 -> 修复动作 -> 重新执行 -> 验证证据”的闭环，且只自动重跑 `repairable=true` 的失败 | task scheduler + UI | retry/reset tests + product path verification | development |
| CF-FR-R3 | 不针对 bench 定制 | 分类规则只读取通用 task 字段：`status`、`error`、`result`、`verification_results`；不得读取 benchmark task name 或 Terminal-Bench artifact path | backend | unit test + code review grep | QA |
| CF-FR-R4 | 用户能区分责任边界 | provider/credential、permission、shell runtime、test failure、verification failure、cancelled、unknown 必须分开展示 | backend + UI | fixture classification tests | QA |
| CF-FR-R5 | 兼容旧数据 | 不要求 SQLite schema migration；旧 `task_runs` 读取后按现有字段派生 attribution | storage + Tauri serialization | cargo test + existing app DB smoke | development |
| CF-FR-R6 | “需要我处理”必须可操作 | 不可自动修复项必须提供直达配置或带证据回到对话的动作；用户确认外部原因已修复后，可显式重试同一会话中选定失败项 | Workspace + Settings + task scheduler | Workspace UI test + selected-retry Rust test + real browser action check | development + QA |

## Primary User Path

P-FR-1: 用户打开 CodeFactory，进入一个 project session。左侧任务列显示任务状态。如果某个任务失败或取消，系统在任务行下方显示失败归因标签和下一步建议。用户点击 `修复可修复项` 后，系统只把 `repairable=true` 的 failed/cancelled 任务重置为 pending 并启动同一 session 的执行；不可自动修复的 provider、权限或运行环境失败必须保持失败状态并提示用户先处理原因，且提供对应设置入口或将失败证据带回对话；只有用户明确确认外部原因已修复后，才能把选定失败项重置为 pending 并重启同一 session。任务重新运行后，状态、验证结果和 evidence pack 继续更新。

P-FR-2: 如果失败来自 provider/credential、权限或 shell runtime，UI 必须让用户看到这不是“模型不会写代码”的同类问题；下一步建议应指向充值/更换模型、授权、配置 PATH/依赖，而不是盲目重试代码。Provider 失败须直达“端点”设置，权限失败须直达“权限”设置，其他阻塞须将任务标题与错误证据预填回当前对话。修复后由用户点击“已修复，重试”触发选定项重试。

## Applicable Harnesses

- Spec Harness: 本规格、Req ID、测试矩阵和证据要求必须随代码提交。
- Compatibility Harness: 不破坏旧 `task_runs` schema、旧 session 和已有任务列表。
- Observation Harness: 失败归因必须来自可审计字段，不得只显示模糊文案。
- AI Collaboration Harness: 归因规则必须标明假设和验证结果；bench 结果只能作为样本，不能成为专用逻辑。
- Release Harness: 发布后必须在安装版 CodeFactory 主路径验证设置、模型选择器、任务修复入口和归因展示。

## Failure Taxonomy

| Kind | 中文标签 | 典型证据 | 默认下一步 |
| --- | --- | --- | --- |
| `model-provider` | 模型/Provider | HTTP 402/429/5xx、insufficient balance、invalid API key、unauthorized、rate limit | 修复 endpoint/key/balance/model route 后重试 |
| `permission` | 权限/策略 | denied、permission、outside cwd、hard deny、用户拒绝 | 调整授权或任务边界后重试 |
| `shell-runtime` | 运行环境 | command not found、No such file、executable unavailable、spawn/ENOENT | 修复 PATH/依赖/命令环境后重试 |
| `test-failure` | 测试失败 | npm test/pytest/cargo test failed、assertion、expected/actual | 基于失败断言修改实现并重跑最小测试 |
| `verification` | 验收失败 | `verification_results` 存在 failed check，或 summary 表示 final verification failed | 读取失败验收项，修实现并重跑同一检查 |
| `cancelled` | 已取消 | status 为 cancelled | 确认任务仍有效后重新执行 |
| `unknown` | 未分类 | 没有足够字段 | 展开任务详情和子会话，补充失败证据 |

## Testing Matrix

| Scenario | Expected evidence |
| --- | --- |
| failed task with failed `verification_results` | classified as `verification`, UI shows `验收失败` and next action |
| provider billing/credential error | classified as `model-provider`, marked not blindly repairable |
| missing command or executable | classified as `shell-runtime` |
| automatic repair loop with mixed failed tasks | only `repairable=true` tasks are reset/re-run; provider/runtime/permission failures stay failed |
| six provider credential failures | drawer exposes endpoint settings and a user-confirmed retry for exactly the six selected failures; actions remain visible at the minimum viewport |
| explicit selected retry | only selected failed/cancelled rows in the same session are reset; completed, unselected and foreign-session rows remain unchanged |
| unknown blocker | drawer returns to the conversation with task title and error evidence prefilled |
| paused pending tasks without failures | drawer explains how many tasks remain and labels the action `继续执行 N 项` |
| pending tasks plus any failed task | drawer explains that failures must be handled first and hides the generic continue action |
| assertion/test failure | classified as `test-failure` |
| cancelled task | classified as `cancelled` |
| old task without new persisted fields | list still loads and derives attribution from existing fields |

## Evidence Pack Requirements

每次交付至少记录：

- classifier unit test results。
- Workspace UI test result（设置路由、显式重试、回到对话三条动作链）。
- `pnpm build` 和治理基线结果。
- 真实安装版或 dev app 主路径截图/观察：任务列中失败归因可见，临时验证数据已清理。
