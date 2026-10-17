---
req_id: CF-CLOSE-OBJ
title: close_objective 在同一会话多个 objective 时选中「已合并待验收」的那条
status: approved
created_at: 2026-10-17
updated_at: 2026-10-17
tags:
  - objectives
  - acceptance
  - dispatch
acceptance_criteria:
  - 同一会话存在多条 objective 时，close_objective 选中唯一一条「已合并、待验收」的记录
  - 同一会话存在多条「已合并、待验收」时要求显式 objective_id，并返回候选列表
  - 桌面命令、本地派单入口（headless dispatch）与会话侧栏入口都接受可选 objective_id
  - 显式 objective_id 不属于该会话或不可结案时明确失败，不误关其他记录
---

# 背景

真机证据（v1.88.9，2026-10-17 12:09）：会话 `cd76d8d2` 下有两条 objective：

- `3749ff64`：`waiting_system` / `delivered_awaiting_ci`，`requested_acceptance=live_verification`，`reached_acceptance=merged`，创建于 09-30 17:56；
- `df8bdbbb`：`failed` / `technical_recovery_exhausted`，创建时间更晚（09-30 20:37）。

本地入口 `close_objective {session_id: cd76d8d2…, outcome: accepted}` 返回
`only a merged task awaiting acceptance can be closed`。原因是解析逻辑按
`ORDER BY created_at DESC` 取会话内最新的一条 objective，命中了早已失败的那条，
而不是等待验收的那条。

# 决策

- 会话级解析不再"取最新一条"，而是取**唯一一条**「已合并、待验收」的 objective
  （`status='waiting_system' AND reached_acceptance='merged'`，或已 `completed` 的
  幂等重放）。
- 候选多于一条时 fail closed：不猜、不按时间挑，要求调用方显式指定
  `objective_id`，并在错误信息里带上候选列表。
- `objective_id` 作为可选参数贯穿 Tauri 命令、headless dispatch 协议和界面入口；
  显式指定时校验归属会话与可结案状态。

# Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| CF-CLOSE-OBJ-R1 | 会话内存在「已失败/已取消」的新记录与「已合并待验收」的旧记录时，close_objective 选中后者并结案 | 合成会话集成测试（红→绿） |
| CF-CLOSE-OBJ-R2 | 会话内存在多条可结案候选时拒绝隐式选择，错误信息包含 `objective_id` 与全部候选 id | 合成会话集成测试 |
| CF-CLOSE-OBJ-R3 | 显式 `objective_id` 只在属于该会话且可结案时生效；跨会话/不可结案明确失败 | 合成会话集成测试 |
| CF-CLOSE-OBJ-R4 | Tauri 命令、headless dispatch 请求体、前端 dispatch 解析与侧栏入口都支持可选 `objective_id` | Rust 协议测试 + 前端 `desktopDispatch.test.ts` |
| CF-CLOSE-OBJ-R5 | 结案仍保持审计事件与幂等语义（重复结案不新增事件、不改 revision） | 既有 M84 测试 + 新测试断言 |

# Applicable Harnesses

- Spec Harness
- Compatibility Harness（旧调用方不带 `objective_id` 必须继续可用）
- Observation Harness（objective_events 审计与候选列表可观察）
- AI Collaboration Harness

# Primary User Path

编排方在锁屏或无人值守场景下，用本地派单入口对**已经合并、等待验收**的会话结案；
会话里曾经失败过的重复 objective 不应影响这次结案。界面里同一入口给出同样的结果
或明确的候选列表错误。

# 测试矩阵

| 层 | 成功路径 | 边界路径 |
| --- | --- | --- |
| 选择 | 唯一「已合并待验收」被选中并结案 | 只有失败记录时不误结案 |
| 歧义 | 显式 objective_id 结案成功 | 多个候选时拒绝并列出候选 |
| 归属 | 同会话 id 生效 | 跨会话 id、不可结案 id 失败且未改动任何记录 |
| 协议 | 旧请求体不带 objective_id 仍可用 | objective_id 为空串被拒绝 |
| 幂等 | 重复结案 revision 不变、只一条事件 | 结案后再次按会话调用仍幂等 |

# Implementation Notes

- 会话级解析在事务内完成；候选排序仅用于稳定输出候选列表，不用于隐式挑选。
- 侧栏入口在同一会话存在多个候选时展示错误信息（含候选 id），由调用方带
  `objective_id` 重试；更细的多候选选择弹层留作后续迭代。
- 真实安装版上的结案验收由编排方在发版后完成：打开会话 `cd76d8d2`，确认
  `3749ff64` 变为 completed 且 `df8bdbbb` 保持 failed。
