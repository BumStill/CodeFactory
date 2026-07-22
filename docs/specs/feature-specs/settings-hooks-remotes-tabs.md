---
req_id: CF-011
title: Settings 页 — Hooks & Remotes 标签页
status: approved
tags: [settings, hooks, remotes, frontend]
acceptance_criteria:
  - SettingsPage 新增 "Hooks" 和 "Remotes" 两个标签页
  - Hooks 标签页：列出现有 hooks，支持启用/禁用/删除/测试，支持添加新 hook
  - Remotes 标签页：列出 GitHub/GitLab remotes，支持添加/删除/测试连接
  - tsc --noEmit 无错误
  - 页面风格与现有 Endpoints / Permissions / General 标签一致（同一设计语言）
---

# CF-011 Settings 页 — Hooks & Remotes 标签页

## 背景

Settings 页重构时（CF-010 之后）将 Hooks 和 Remotes 管理从旧 modal 移走，
现在需要将它们重新实现为 SettingsPage 的两个正式标签页。

## 目标

在 `src/pages/Settings/SettingsPage.tsx` 中新增两个标签页，
把原本在 ChatPage 里的 HooksSection / RemotesSection 的功能搬进来，
保持与 Endpoints / Permissions / General 一致的设计风格。

## 现有代码位置

- SettingsPage: `src/pages/Settings/SettingsPage.tsx`
- 旧 Hooks 代码（已删除）: 曾在 ChatPage.tsx 的 HooksSection、AddHookForm
- 旧 Remotes 代码（已删除）: 曾在 ChatPage.tsx 的 RemotesSection、AddRemoteForm
- gitRemote store: `src/stores/gitRemote.ts`
- Tauri 命令: list_hooks, add_hook, update_hook, delete_hook, test_hook
- Tauri 命令: list_git_remotes, add_git_remote, delete_git_remote, test_git_remote

## Tab 类型扩展

现有 Tab 类型：`"endpoints" | "permissions" | "general"`
扩展为：`"endpoints" | "permissions" | "general" | "hooks" | "remotes"`

## Hooks 标签页功能

- 加载并展示所有 hooks（invoke list_hooks）
- 每条 hook 显示：name、event、action type、enabled/disabled toggle、test 按钮、删除按钮
- 测试结果内联显示在 hook 卡片下方
- "Add Hook" 按钮打开 inline form，字段：name, event (select), action type (select), action param, filter
- Event 选项：pre_tool, post_tool, pre_task, post_task, session_start, session_end, spec_approved, verification_failed
- Action 类型：log_to_file, run_command, emit_event, auto_git_commit

## Remotes 标签页功能

- 加载并展示所有 Git remotes（useGitRemoteStore）
- 每条 remote 显示：provider badge、name、default_repo、test 按钮、删除按钮
- "Add Remote" 按钮打开 inline form，字段：name, provider (github/gitlab), base_url, token, default_repo
- 自动填充 base_url（github → https://api.github.com，gitlab → https://gitlab.com/api/v4）

## 任务分解

### Task 1: 扩展 Tab 类型和标签栏
依赖：无
- 在 SettingsPage.tsx 中把 Tab 类型加上 "hooks" | "remotes"
- 在 tabs 数组里加入 { id: "hooks", label: "Hooks" } 和 { id: "remotes", label: "Remotes" }
- 验证：tsc --noEmit 通过

### Task 2: 实现 HooksTab 组件
依赖：Task 1
- 在 SettingsPage.tsx 内实现 HooksTab 组件（可以是 local function）
- 功能：list/toggle/delete/test/add
- 在 Tab === "hooks" 时渲染
- 验证：tsc --noEmit 通过

### Task 3: 实现 RemotesTab 组件
依赖：Task 1
- 在 SettingsPage.tsx 内实现 RemotesTab 组件
- 使用 useGitRemoteStore
- 功能：list/delete/test/add
- 在 Tab === "remotes" 时渲染
- 验证：tsc --noEmit 通过
