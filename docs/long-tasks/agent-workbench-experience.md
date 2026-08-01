# 现代 Agent Workbench 长任务记录

## Basics

- Task ID: CF-WB-20260730
- Title: Workspace 现代视觉与作业流演进
- Feature spec: `docs/specs/feature-specs/agent-workbench-experience.md`
- Related Req IDs: CF-WB-R1..R18

## Completion Standard

- Done means: CF-WB-R1..R18 均有实现与证据；PR+CI 合并；按刻意发版节奏进入公开安装产物；真实 CodeFactoryDev 与公开产物主路径通过。
- Blocked means: 同一外部阻塞连续三个 goal turn 无法推进，且已记录阻塞证据、责任人和下一步。

## Current State

- Current phase: 第一批 — 视觉与任务真相基础
- Current checkpoint: 第一批已随 v1.74.0 发布；用量/context 上移已随 v1.75.1 发布；输入控件对齐和会话侧栏归属修正已随 v1.76.1 发布，并完成公开安装产物与 GUI 验收。
- Next owner: 第二批继续 R17 单一 pane arbiter 与 R18 共用任务活动组件。
- Updated at: 2026-08-01

## Completed Items

- [x] 隔离 worktree，未触碰 main 上的按需浏览器 staged 改动。
- [x] 业务、架构、UX 与 feature spec 落库。
- [x] 明确外部机器人不属于产品。
- [x] 裁决结果 footer、release/live、token footer 和右侧 pane 冲突。
- [x] 先写视觉契约、结果、context、queue、sidebar、抽屉与 delivery 失败验收，并确认修复前失败。
- [x] 建立 light/dark surface、semantic status、15/13/11px 字号与 WCAG AA 对比度门禁。
- [x] 实现 880px 阅读列、统一 raised composer、真实 context 阈值、queue disclosure 与失败优先结果快照。
- [x] 实现会话搜索、完整标题、无嵌套交互结构，以及 ≤720px overlay 侧栏。
- [x] 实现任务/交付抽屉的 `aria-expanded`、初始聚焦、focus trap、Escape 关闭与回焦。
- [x] 固定区分 PR、CI、merge、formal release 和 live verification；Settings/Onboarding 不再把 release 写成上线。
- [x] 全量 89 个测试文件 / 447 项测试、TypeScript production build、治理基线与长任务结构 validator 通过。
- [x] 真实隔离 Tauri 在 800×600 浅色/深色完成成功与失败路径、两次 `update_plan`、结果持久化、抽屉键盘和无横向溢出验收。
- [x] 清理本任务启动的 app process group、1420/18765 端口、fixture server、测试 task row、wrapper 与隔离 HOME。
- [x] 第一批由 PR #270 合并，并随 v1.74.0 生成 macOS/Windows 安装产物与 `latest.json`。
- [x] 用户反馈的用量/context 位置由 PR #272 修正，并随 v1.75.1 发布。
- [x] PR #274 将侧栏收起控件归还“会话”栏头，并统一曲别针、单行文字与发送按钮的 32px 垂直基准；92 个测试文件 / 472 项测试、build、治理、远程 GUI 与本地真实 App 验收通过。
- [x] PR #274 所在批次随 v1.76.1 发布；macOS/Windows 构建、Windows Evolution 闭环、已安装 macOS 候选和重新下载的公开 DMG GUI 验收均通过。

## Remaining Items

- [ ] 补齐 1366×768 与 200% zoom 独立截图证据；≤720px 行为当前由真实 800×600 + `matchMedia` component test 覆盖。
- [ ] 第二批：R17 单一右侧 pane arbiter，合并任务、Git、交付、证据和按需浏览器入口。
- [ ] 第二批：R18 Workspace/acceptance 共用 TaskActivityDrawer，并清理旧 TaskDashboard/ExecutionStream 虚假验收面。
- [ ] 无障碍 P2：补齐消息/工具/任务 disclosure 的 `aria-controls`，项目选择与会话菜单键盘模式，以及 ImagePreview 初始聚焦、focus trap、回焦和最小 11px 说明。
- [ ] 测试/性能 P2：清理 Workspace `act(...)` 警告，并单独治理 Vite 既存大 chunk 提示。

## Blockers

- None

## Evidence

- Local evidence:
  - `pnpm test`：89 files / 447 tests passed。
  - `pnpm build`：TypeScript + Vite production build passed。
  - `python3 tools/governance/validate_repo_governance_baseline.py`：pass。
  - `python3 tools/governance/validate_long_task_record.py --task-record-path docs/long-tasks/agent-workbench-experience.md`：pass。
  - 真实隔离 Tauri：固定本地 fixture endpoint 下，两步计划从 `completed/in_progress` 演进到 `completed/completed`，两次 `update_plan` 均为 `done`，切换会话后结果快照仍存在。
  - 真实边界路径：OpenRouter 不可用时显示“需要处理”而非成功；交付 remote unavailable 不伪造 PR/CI/release/live。
  - 可访问性树：任务与交付抽屉 Escape 关闭后焦点分别返回触发按钮。
- Release evidence:
  - v1.74.0 与 v1.75.1 均为公开非预发布 release，包含 macOS/Windows 安装产物、签名和 `latest.json`。
  - v1.76.1 为公开非预发布 release；macOS DMG、Windows NSIS、签名与 `latest.json` 均可公开下载，HTTP 返回 200。
  - `latest.json` 的版本为 `1.76.1`，Windows x86_64 / NSIS 与 Darwin arm64 URL 均指向 v1.76.1 且签名非空。
  - Release run 30690692975 全部绿色；公开 DMG 被重新下载并通过安装产物与 GUI 验收。
- Blocking evidence: 当前无。

## AI Collaboration

- context scope: Workspace theme、session rail、conversation、result、composer、context、task activity、delivery、Settings/Onboarding delivery copy。
- assumptions: 第一批不修改持久化 schema；release snapshot 当前没有 live verifier 字段；外部机器人不属于产品。
- review point: 独立 QA 先后阻止低对比 muted/status 色、嵌套交互、抽屉焦点、窄屏、9–10px、hardcoded 状态色和 reduced-motion 缺口进入交付；逐项修复后复审。
- validation result: 第一批与两轮用户反馈均已公开发布；PR #274 的全量测试、build、治理、本地真实 App、远程 GUI 与 v1.76.1 公开产物验收通过；R17/R18 和补充视口证据仍待后续批次。

## Stop Boundary

- 不在单元测试或 build 后停止。
- 不在 PR、CI 或正式 release artifact 后把未验证 live 写成上线。
- 只有完成全部证据，或达到有证据的阻塞条件时停止。
