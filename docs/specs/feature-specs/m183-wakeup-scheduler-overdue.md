# M183（紧急，架构）：唤醒调度器不执行已过期的 pending 唤醒——「该醒不醒」家族根因

## 证据（v1.91.43，10-27 00:00，编排方直接查库）
- `wakeup_schedules` 中 19 条 status=pending、fire_count=0 的唤醒 **next_at 早已过期**仍未执行，最早：
  - 019a75d8 next_at 2026-10-24 01:56「PR #857 被合并队列退回…」
  - b88017e6 next_at 2026-10-24 03:21「PR #859 已合并…执行 §8 收尾」
  - aeba516d 2026-10-26 18:00「当前没有可用模型…」；d1cd3201 ×4、ae19592c ×4、954eb4a1 ×3、587c7b0c、af56308a、9e93b6a6 ×2（10-26 20:11 ~ 23:51）
- 同时 14 个 PR 已合并的会话 thin_session_state 显示 `waiting_wake / wake_source=持久收尾投递器：系统自动重试/自愈 / reason=PR #N 已合并；接续收尾`，但 **wakeup_schedules 里没有对应的待执行唤醒行**（如 c8e12ca7/#872、98b26375/#876）——状态宣称有唤醒源，实际没有。
- 结果：M168（唤醒可靠送达）、M174（合并后收尾）在真机都不生效；编排方反复手动叫醒（10-22 ~ 10-26 至少 10 次）。
## 需求
- R1 查清调度器为何不触发过期 pending（只调度「未来」时间？重启后未加载？单位秒/毫秒混用？被某个单会话阻塞？），写进 PR。
- R2 单一调度器：启动时与运行中周期性（≤30s）扫描所有 `status=pending AND next_at<=now` 的唤醒并执行（至少一次、幂等、按会话串行）；过期很久的也要执行（不丢弃），对同一会话的多条过期唤醒合并为一次唤醒并带上全部原因。
- R3 不变量：任何会话 thin=waiting_wake 必须对应 ≥1 条 pending 唤醒行（或活的后台任务/PR 监视登记）；否则启动时补建唤醒或改为 waiting_user 并给原因。M174 的「持久收尾投递器」必须真正写入并执行唤醒。
- R4 监控：pending 过期 >2 分钟未执行 → 记录告警并自愈重试；在会话诊断里可见。
- R5 测试（先失败，跨进程）：写入过期 pending 唤醒后启动 app → 30s 内被执行；运行中到点唤醒被执行；PR 合并后会话被唤醒收尾并 ended；waiting_wake 无唤醒行的存量会话在启动时被补建/修正。
- R6 真机验收：装新版后，上述 14 个存量会话与 19 条过期唤醒被处理（结束或执行），附查询结果。
## 约束
- task-template；合成 fixture；授权交付（ceiling: merged）。

Release-Urgency: immediate

## Implementation Notes

- 不改生产数据；R6 由编排方按 task-template §8 在发版安装后查询验收。本任务交付至队列合并，不触发发版。
- 测试仅使用合成 SQLite 与子进程。无 UI 变更，不启用 Viewport Harness。
- Applicable Harnesses：Spec、Compatibility（持久 SQLite / 重启）、Observation（过期诊断）、Release（合并与发版交接）、AI Collaboration（单执行流，遵循任务模板禁止 delegate_tasks）。
- Req ↔ 测试和验收结果随实现补充在本文件与 PR；安装版验收不得用 headless 测试替代。
