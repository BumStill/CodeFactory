# 仓库归属的规范与会话内计划

> Req ID: `CF-REPO-SPECS`
>
> 状态: approved
>
> 决策日期: 2026-07-22

## 背景

CodeFactory 曾把规范编辑、AI 生成、任务拆解和远程 Issue 导入组合成独立的
`SpecsPage` 产品模块，并把文档写入 `.codefactory/specs/`。这使工作区顶栏承担了
一个脱离当前会话的“规范工作台”，同时让 `docs/specs/` 与
`.codefactory/specs/` 成为两个竞争的权威来源。

本规格确立三个不同的所有权层级：长期意图属于代码库；本轮计划和任务状态属于
会话执行；模型、权限和个人偏好属于用户设置。CodeFactory 不再拥有独立的规范
内容层。

## Requirements Traceability

| Req ID | 用户要求 | 规范化要求 | Surface | 验证方法 | Owner |
| --- | --- | --- | --- | --- | --- |
| CF-REPO-SPECS-R1 | 删除规范和计划入口 | 工作区不得出现独立“规范工作台”“拆任务”或空任务规划面板 | Workspace | Vitest + 真实 App 顶栏 | frontend |
| CF-REPO-SPECS-R2 | 计划融入会话执行内部 | 复杂请求由会话 Agent 使用 `delegate_tasks` 创建任务；只有任务存在时才显示执行详情 | Agent + Workspace | 工具测试 + Workspace 测试 | agent |
| CF-REPO-SPECS-R3 | 内容随代码库存在 | 长期规范和设计以普通、可版本化的仓库文件存在，默认权威入口为 `docs/specs/` 与 `docs/design/` | Agent context + Git | Rust 测试 + Git diff | agent |
| CF-REPO-SPECS-R4 | 不随 Agent 产品模块存在 | 移除 Specs CRUD/AI/decompose Tauri commands、SpecsPage/store 和 Issue→Spec 专属入口；runtime 不再写 `.codefactory/specs/` | frontend + Tauri | 静态检索 + typecheck + Rust build | development |
| CF-REPO-SPECS-R5 | 旧内容不能丢 | 已跟踪的 `.codefactory/specs/*.md` 迁入 `docs/specs/feature-specs/` 并继续由 Git 管理 | repository | `git diff --summary` + 文件读取 | development |
| CF-REPO-SPECS-R6 | 历史任务和证据不能坏 | 保留 nullable `task_runs.spec_req_id/spec_title`、Evidence Pack manifest 和历史 UI provenance | SQLite + evidence | 相关 Rust/TS 回归测试 | compatibility |
| CF-REPO-SPECS-R7 | 只有一个规范权威面 | Control Plane 只把 `docs/specs` 列为规范权威，不再把 `.codefactory/specs` 当作能力面 | Control Plane | Rust 单元测试 | governance |

## Primary User Path

1. 用户打开一个项目会话并描述非平凡代码任务。
2. CodeFactory 向会话 Agent 提供有界的仓库权威索引：先读取 `AGENTS.md` 与 README，
   并发现仓库已经采用的 `docs/specs/`、`docs/design/` 等普通文档；仓库规则决定最终路径。
3. Agent 读取与任务相关的仓库规范；执行期任务状态只在当前对话中表达，不创建独立工作台记录。
   超过一屏的长方案按 `planning-turn-document-authoring.md`（`CF-PLAN-DOC`）落盘为仓库文档，
   会话只保留摘要、文档路径和待决问题。
4. 如任务可以安全并行，Agent 调用 `delegate_tasks`；任务存在后，执行详情在会话内部浮现。
5. 如本次决策应长期保留，Agent 直接创建或修改 `docs/specs/`、`docs/design/` 等普通
   仓库文件；这些变更与代码一起进入 diff、commit、PR 和 Git 历史。
6. 用户始终从会话、diff 与交付证据审查结果，不需要进入规范/计划产品页面。

## Applicable Harnesses

- **Spec Harness**：本文件及三份设计文档是 CodeFactory 仓库本次实现的权威；产品不把
  这一目录结构强加给其他仓库。
- **Compatibility Harness**：保留历史任务、Evidence Pack 与旧数据库字段；迁移已跟踪文档。
- **Viewport Harness**：验证顶栏移除入口后在常规与窄窗口均无空洞、重叠或隐藏主操作。
- **AI Collaboration Harness**：验证 Agent 能发现仓库权威，计划与任务委派保持会话内生。
- **Observation Harness**：记录实际 Agent prompt/工具路径和真实 App 顶栏证据。

## 测试矩阵

| 场景 | 期望结果 | 证据 |
| --- | --- | --- |
| 项目没有任务 | 顶栏无规范/拆任务，执行详情不出现 | Workspace Vitest + App 截图 |
| Agent 委派复杂任务 | 当前会话出现任务状态，用户无需打开独立页面 | delegate_tasks + Workspace tests |
| 仓库包含 `docs/specs/feature-specs/x.md` | Agent prompt 可发现相对路径并要求按需读取 | Rust unit test |
| 仓库仅有旧 `.codefactory/specs` | 不再被 Control Plane 或 Agent 当作规范权威 | Rust unit test |
| 历史 task 含 `spec_req_id/spec_title` | 任务和 Evidence Pack 仍能读取历史 provenance | existing regression tests |
| 查看远程 Issue | 不出现“创建为规范”独立 CTA | component test / App check |
| 1366x768 与窄窗口 | 顶栏无遗留空位或重叠 | headless + CodeFactoryDev |

## 兼容与迁移

- 不删除、不重命名 SQLite 中的 `spec_req_id`、`spec_title` 字段。
- 不修改既有 Evidence Pack 目录、manifest 和事件格式。
- 已跟踪的 `.codefactory/specs/*.md` 通过 Git move 迁入
  `docs/specs/feature-specs/`；不扫描或静默搬运用户其他仓库中的本地文件。
- 旧 `spec_approved` hook 字符串继续可反序列化，本轮不做破坏性配置迁移；该事件已无新 UI
  触发点，后续可单独废弃。

## Evidence Pack Requirements

- 失败→通过的 Workspace 与 Agent authority 单元测试。
- `typecheck`、相关 Vitest、Rust focused tests、前端 build 和治理 validator。
- 真实 CodeFactoryDev 或 headless 运行中，验证顶栏无规范/计划入口，委派任务仍在会话内显示。
- PR/CI 状态以及堆叠基线的明确说明；未合并、未发布时标记 `not live`。
