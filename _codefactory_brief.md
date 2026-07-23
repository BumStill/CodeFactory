# CodeFactory Shared Brief
Session: 43ec91cc-9b2d-4213-b8a7-a1592acf8bc5

## Parallel Tasks

### Task 1 — 独立 UX 审查
只读审查当前分支 fix/workspace-collapsible-session-rail 的 WorkspacePage、相关测试和规格。不要编辑文件。重点检查用户报告的两个加号是否去重、侧栏收起/展开交互、aria 属性、localStorage 持久化和匿名会话退出路径。运行必要的聚焦测试并返回证据。

### Task 2 — 浏览器视口 QA
只读 QA 当前分支的 Workspace 会话侧栏改动。优先使用仓库现有 Playwright/headless harness；不要编辑源码。验证桌面视口下唯一新建入口、侧栏收起与恢复、主区域宽度/溢出边界。若现有 harness 无法直接覆盖，明确最小缺口与可执行替代证据。

### Task 3 — 任务抽屉独立审查
只读审查当前分支 feat/workspace-attention-hierarchy 的 WorkspacePage 任务活动入口与抽屉实现及测试。不要修改文件。重点检查任务状态摘要、失败可见性、焦点管理、Escape/overlay 关闭、会话切换、deep link、移动/窄视口。运行必要的聚焦测试并返回证据。

### Task 4 — 侧栏与命令层级审查
只读审查当前分支的 SessionSidebar、ToolCallCard、MessageList 与相关测试/规格。不要修改文件。检查时间分组、行高、键盘菜单、新建菜单、工具标签、文件打开入口、成功分组、错误首行、hydrated/live timeline 行为。运行必要测试并返回证据。

### Task 5 — 审查 GitHub 交付状态数据链
只读审查当前分支 feat/workspace-delivery-status 的后端交付状态实现，重点检查 src-tauri/src/commands/git_remote.rs、src-tauri/src/tools/delivery.rs、src-tauri/src/storage/db.rs。不要改文件。验证会话到 PR 的持久关联、GitHub PR head SHA check-runs、merged 字段和 latest release 包含 merge commit 的判定。

### Task 6 — 审查状态栏信息架构与窄屏
只读审查当前分支的前端 UX 实现和测试，重点检查 src/components/GitStatusBar.tsx、WorkspaceDeliveryStatus.tsx、CheckpointsPanel.tsx、src/pages/Workspace/WorkspacePage.tsx 及对应测试。不要改文件。

### Task 7 — 独立 UX 与动作链审查
审查当前分支 fix/task-blocker-actions 的前端改动。重点检查 src/App.tsx、src/pages/Settings/SettingsPage.tsx、src/pages/Workspace/WorkspacePage.tsx、src/pages/Workspace/TaskCreator.test.tsx、src/acceptance/repository-intent.tsx 与 scripts/verify-repository-intent-headless.mjs。用户反馈是“需要我处理”抽屉没有可执行动作；实现目标是模型 Provider 失败提供打开 endpoints/API key 设置与显式重试，未知阻塞回到对话并预填证据。只读审查，不要编辑。

### Task 8 — 独立重试边界审查
审查当前分支 fix/task-blocker-actions 的后端和 store 改动。重点检查 src-tauri/src/storage/tasks.rs、src-tauri/src/commands/tasks.rs、src-tauri/src/lib.rs、src/stores/tasks.ts。只读审查单任务/多任务显式重试的数据安全、会话边界、状态约束和命令 wiring，不要编辑。

## Task Results

_(will be updated as tasks complete)_
