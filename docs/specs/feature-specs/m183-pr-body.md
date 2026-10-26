# fix(runtime): 修复过期唤醒饥饿与无来源等待 (M183)

## 根因与实现
- R1：时间为秒，SQL 原本已包含过去时间，启动也会扫描；不是未来时间过滤。实际扫描先逐行等待后台投递，再逐行等待最多 32 条定时投递，每条可阻塞 5 秒；前方失败行反复占据批次，会使后续会话饥饿。入队后的 CAS 只能去重收据，不能阻止并发扫描重复副作用。
- R2：扫描全部到期 pending，按会话合并全部原因，跨会话并发，扫描前按 pool 排他；成功后推进，失败保持 pending。后台结果投递并发，避免阻断下一次扫描。
- R3：启动对没有真实唤醒来源的 waiting_wake 补建幂等持久唤醒；已有 PR 监视继续复用。保留 M174 事务写入收尾唤醒与去重事实。
- R4：超期两分钟记录日志及 wakeup_alerts；诊断返回超期数量、最长延时与持久重试次数，不导出提示正文。

## Req ↔ 验证
- R2/R5：m183_cross_process_overdue_wakes_are_coalesced_without_starvation（真实 SQLite、子进程、40 条同会话历史积压、另一会话、运行中未来到点、重复扫描）。
- R2：m183_blocked_sessions_do_not_delay_a_healthy_session、m183_concurrent_scans_enqueue_a_session_only_once。
- R3：m183_startup_repairs_waiting_wake_without_a_source。
- R4：m183_diagnostics_expose_overdue_wakes_without_prompt_content。
- R5：已有 m174_cross_process_merged_wait_is_closed_out 覆盖合并→唤醒→最终回答→ended。
- TDD：上述五个 M183 用例已见修复前失败；一次性 fixture 缺少合法延时先修正后重跑，仍在目标断言处失败。
- 前端 pnpm test / pnpm build 已通过。最终 Rust 完整 workspace（主库 2021 passed、12 ignored；集成与其他 crates 全通过）退出码 0；包含 m174_cross_process_merged_wait_is_closed_out。治理基线、验收账本、场景及 README 校验通过。

## Harness / AI Collaboration
Spec、Compatibility、Observation、Release、AI Collaboration；无 UI 布局变更，不适用 Viewport。
context scope：background_tasks / thin_derive / objective_diagnostics；assumptions：仅合成 fixture，不改生产库；review point：跨会话饥饿、入队前去重、持久补建；validation result：以实际命令记录为准。
依任务模板禁止 delegate_tasks，本任务单执行流。

## 真实安装版验收交接（R6）
真机验收由编排方发版后完成，当前 not live；本 PR 不发版。
1. 安装新版并启动，30 秒后查询 `wakeup_schedules WHERE status='pending' AND next_at<=strftime('%s','now')`，记录原 19 条的 status/fire_count/last_fired_at。
2. 检查原 14 个已合并会话，核对真实 pending 收尾行、最终 §8 回答及 ended。
3. 导出会话诊断，核对 overdue_wakeups / oldest_wakeup_delay_secs / wakeup_retry_count；阻塞项应保留 pending 并自动重试。
4. 确认无 waiting_wake 缺 pending/活后台任务/PR 监视来源；附脱敏聚合查询结果。

Scenario-Test: RTE-005, RTE-004, HLT-001, HLT-004, RTE-003
README-Update: reviewed
README-Update-Reason: 修复内部持久唤醒可靠性，不改变公开安装、隐私或平台承诺。

Release-Urgency: immediate
