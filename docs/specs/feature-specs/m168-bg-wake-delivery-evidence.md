# M168 验收与交接

实现：
- `deliver_wake` 为后台任务和窗口唤醒共用有界投递入口，超时保留 pending，五分钟以上记录告警并立即重试。
- 死亡进程无退出收据按 lost 结算并唤醒，不再凭输出文件推断成功。
- PR 观测把全绿、失败、退队/冲突、关闭、合并都作为接续条件；全绿不会伪装成合并完成。
- 有 pending 唤醒时 thin 派生为 waiting_wake；真实问题、停止、最终回答仍优先。

验收用例：
- `m168_a_blocked_sink_cannot_stall_pending_wakes`：投递器不被单会话阻塞，后续重试成功。
- `m168_all_green_ci_is_a_wake_condition`：全绿触发，空检查与未完成不触发。
- `m168_cross_process_restart_delivers_background_and_window_wakes`：真实子进程重开 SQLite，后台任务判 lost 与窗口唤醒各投递一次，父进程确认不重复。
- `m168_pending_wake_without_executor_is_not_running`：pending 状态不再返回 Running。
- 既有事实优先用例：真实提问为 WaitingUser、最终回答仍为 ended。

Harness：Spec、Compatibility、Observation、Release、AI Collaboration。
原始规格：docs/specs/feature-specs/m168-bg-wake-delivery.md。
真机验收由编排方发版安装后完成；本任务止于合并，not live。

Spec Amendment: 无。
Scenario-Test: HLT-001, HLT-004, RTE-005, CXD-002
README-Update: reviewed
README-Update-Reason: 内部可靠性修复，不改变公开产品、安装或隐私承诺。

Release-Urgency: immediate
