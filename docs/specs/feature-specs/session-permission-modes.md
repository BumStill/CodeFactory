---
req_id: CF-SPM
title: 会话级权限模式
status: approved
tags: [permissions, session, ux, security]
acceptance_criteria:
  - 权限不再作为 Settings 的独立页签展示给普通用户
  - 每个会话持久化一个 permission_mode，默认 standard
  - 会话顶栏可选择 safe / standard / trusted 三种权限模式
  - 工具明细列表隐藏在后台预设中，不要求用户维护 allow/ask/deny 列表
  - 后端工具权限决策读取当前会话的 permission_mode，而不是全局 Settings.permissions
  - 高风险和永久拒绝命令在 trusted 模式下仍不可静默放行
---

# CF-SPM 会话级权限模式

## 背景

旧权限页要求用户维护工具 allow / ask / deny 明细，并且设置是全局的。用户实际需要的是“当前会话允许 agent 自主到什么程度”，不是学习工具内部名称。

## 权限模式

- safe：安全确认模式。读取类工具自动允许；写入、编辑、命令和其他副作用工具询问。
- standard：标准模式。常规文件与文档工具自动允许；shell 命令询问。
- trusted：信任模式。常规工具和普通命令自动允许；内置高风险命令与永久拒绝规则仍然询问或拒绝。

## 主路径

用户在会话顶栏看到“会话权限”控件，选择本会话的权限模式。切换只影响当前会话，从下一次工具权限判断开始生效；不改变消息是分析还是执行。
