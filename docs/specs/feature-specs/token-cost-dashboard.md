---
req_id: CF-010
title: Token 成本仪表盘
status: superseded
tags: [cost, analytics, dashboard]
acceptance_criteria:
  - 每次 AI 响应完成后，input/output token 数与成本被写入 cost_entries 表
  - ChatPage 顶部显示"本会话 token"、"今日 token"、"本月成本（USD）"三个指标
  - 数据实时更新（每次对话结束后自动刷新）
  - 支持多 endpoint / model 的成本分开记录
  - 月度成本按 OpenRouter 定价公式估算（$1 = 1M token 作为默认，可被覆盖）
---

> 历史规格：本文件的采集目标已由 `token-usage-dashboard.md` 继承；ChatPage 顶部三个常驻指标和旧 `TokenCostBar` 位置已被 CF-USAGE-R13/R14 与 CF-WB-R19/R21 取代。当前权威交互是在 composer 常驻 context 圆环，累计 Token/成本进入点击详情与「用量与预算」页。

# CF-010 Token 成本仪表盘

## 背景

CodeFactory 的用户需要了解 AI 调用的 token 消耗与成本，以便合理控制开支。
当前系统缺少任何 token 追踪机制，所有成本对用户不透明。

## 目标

在 ChatPage 顶部增加一个轻量的成本状态条，展示关键 token 指标；
后端记录每次 AI 响应的 token 使用量与估算成本。

## 数据模型

### cost_entries 表

```sql
CREATE TABLE IF NOT EXISTS cost_entries (
    id          TEXT PRIMARY KEY,           -- UUID v4
    session_id  TEXT NOT NULL,
    model       TEXT NOT NULL,
    endpoint    TEXT NOT NULL,
    input_tokens  INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cost_usd    REAL NOT NULL DEFAULT 0.0,  -- 估算值
    created_at  TEXT NOT NULL               -- ISO 8601
);
```

## 后端命令

| 命令 | 说明 |
|------|------|
| `record_token_usage` | 记录单次 AI 响应的 token 使用（由 agent 响应结束时调用） |
| `get_session_cost` | 查询指定 session 的累计 token 与成本 |
| `get_today_cost` | 查询今日所有 session 的累计 token 与成本 |
| `get_monthly_cost` | 查询本月累计成本（USD） |

## 前端组件

### TokenCostBar

位置：ChatPage 顶部工具栏区域（ModelPicker 右侧）

显示内容：
- 📊 `本会话: 12,345 tok`
- 📅 `今日: 89K tok`  
- 💰 `本月: $0.42`

行为：
- 每次 session_id 变化时重新拉取本会话数据
- 每次收到 `token-usage-recorded` 事件时刷新
- 点击展开详情弹窗（可选，暂不实现）

## 成本估算

默认定价（可在 settings.json 中覆盖）：
- input: $1 / 1M tokens
- output: $3 / 1M tokens

## 任务分解

### Task 1: DB migration
依赖：无
- 创建 `migrations/0005_cost_entries.sql`
- 验证：文件存在且包含正确 CREATE TABLE

### Task 2: 后端 commands/costs.rs
依赖：Task 1
- 实现 4 个 Tauri 命令
- 在 commands/mod.rs 注册
- 验证：cargo check 通过

### Task 3: Agent 响应后自动记录 token
依赖：Task 2
- 在 `agent/mod.rs` 的 `run_openai()` 和 `run_anthropic()` 响应处理中
  提取 usage 字段并调用 record_token_usage
- 发射 `token-usage-recorded` Tauri event
- 验证：cargo check 通过

### Task 4: 前端 TokenCostBar 组件（历史实现，已被新信息架构取代）
依赖：Task 2, Task 3
- 创建 `src/components/TokenCostBar.tsx`
- 集成到 ChatPage 顶部工具栏
- 验证：tsc --noEmit 通过

### Task 5: 端到端验证 & 样式调整
依赖：Task 4
- 验证完整链路：对话 → token 写库 → 组件实时更新
- 确认三个指标均正确显示
