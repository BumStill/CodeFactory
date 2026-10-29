# M199（紧急）：「等待 §8 收尾」写了一个不存在的唤醒源——M197 上线后仍有 31 个会话永久挂起

## 本机证据（v1.91.55 装好 5 分钟后，10-29 15:30）
- 37 个 waiting_wake，其中 31 个 `wake_source='持久收尾投递器：系统自动重试/自愈'`、`waiting_for='PR #xxx 已合并，等待 §8 收尾'`（PR #123、#818…#912），`wakeup_schedules` 里**没有任何 pending**。
- 例：会话 de8a380d（PR #908，10-29 10:20 已合并）：`thin-closeout-*`、`thin-closeout-source-*` 两条唤醒都是 `cancelled`、`fire_count=0`——收尾一次都没执行就被取消。`thin_external_observations.closeout_queued=1`。
- 所有 31 条 `updated_at` 都是 15:30:11（同一次周期扫描），说明扫描每轮都把它们重新投影成「等收尾」，但从不真正挂唤醒。

## 从代码推断的根因（实现前先用 fixture 复现确认）
- `thin_derive.rs` 已合并 PR 分支（约 712 行）直接返回 WaitingWake + 字符串唤醒源「持久收尾投递器」，**不检查该投递器有没有 pending 唤醒**——只是个名字。
- `write_state_and_cancel`（约 2176 行）每次落状态都会把会话所有 pending 唤醒置 cancelled；收尾唤醒因此被结算/提问路径取消，之后派生又投影回「等收尾」。
- M197 的 `is_system_followed` 把「系统自动重试/自愈」文案当作已被跟进，所以对账认为有源、不补唤醒。

## 需求
- R1 不变量（补全 M197）：唤醒源必须是**可查询的事实**（pending wakeup 行 id、running 后台任务 id、外部观察订阅），不能是文案。派生写 waiting_wake 时同一事务内确保对应 `thin-closeout-*` 唤醒存在（幂等）；对账判断"有源"只认事实，不认文案。
- R2 收尾有界：每个已合并 PR 的收尾最多尝试 N 次（建议 2）；成功 → ended；耗尽或会话的工作区/分支已被清理（收尾已无事可做）→ ended，reason 写人话（「PR #908 已合并；收尾已由…完成/无需收尾」），不得转成问用户。
- R3 取消唤醒不得误杀收尾：`write_state_and_cancel` 只在结案/明确停止时取消；写 waiting_wake 的路径不得取消自己刚需要的唤醒。
- R4 存量修复：启动与周期对账把现有 31 个按 R1/R2 处理（预期：大部分直接 ended，其余挂一次收尾唤醒），幂等，记 session_audit_events。
- R5 回归测试（先失败，合成 fixture）：PR 已合并 + 收尾唤醒已 cancelled 的会话 → 一次对账后要么有 pending 收尾唤醒、要么 ended，绝不出现"waiting_wake 且无 pending"；跑两次对账结果不变。登记到 `docs/testing/scenario-mappings.json`（append-only）。

## 约束
- 恢复/等待回路同族第 N 次复发：PR 描述里列出所有写 waiting_wake 的入口及各自的**事实型**唤醒源与上限。
- task-template（harness + SDD + TDD）；合成 fixture；不改生产数据。授权交付（ceiling: merged）。

Release-Urgency: immediate

## Implementation Notes
- 需求追踪：R1/R3 对应事务式收尾投影与取消边界；R2 对应当前 PR 的累计 fire_count 上限 2 和工作区不存在事实；R4/R5 对应启动/周期共用对账、审计及双次幂等 fixture。
- Applicable Harnesses：Spec、Compatibility（旧库与 cancelled 存量）、Observation（字段断言与审计）、Release（合并交接，不在本任务发版）、AI Collaboration（独立只读审查）。无 UI 布局变更，不触发 Viewport。
- 测试矩阵：cancelled/无源、pending 幂等、两次耗尽、工作区已清理、成功收尾、提问不误杀、运行执行者保护；所有 fixture 为合成 SQLite。
- 真实安装版验收由编排方在单独发版安装后执行：启动后核对历史已合并等待会话、pending 唤醒行、收尾次数、session_audit_events；180 秒后复核无无源 waiting_wake，不修改生产数据。
