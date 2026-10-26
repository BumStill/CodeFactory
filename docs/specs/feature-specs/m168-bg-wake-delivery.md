# M168：后台任务结束后的唤醒必须送达

## 原始需求（任务文件）
- R1 唤醒投递是持久的、至少一次：background_tasks/PR 监视/定时唤醒产生的唤醒在落库后由同一投递器处理，失败重试；启动时扫描所有 pending 唤醒并投递（覆盖 app 在任务结束前后重启的情况；进程已死的后台任务按 lost 结算并唤醒）。
- R2 PR 监视（M158）把 CI 结论（失败/全绿）、被队列退回、冲突、关闭、合并都作为唤醒条件。
- R3 窗口/额度等待（M164）到点唤醒走同一投递器，跨重启有效。
- R4 不变量 + 监控：任何 pending 唤醒超过 5 分钟未投递 → 记录并立即重试；thin=running 但无活 executor 且有 pending 唤醒的状态不得持续。
- R5 测试（先失败，跨进程）：后台任务运行中重启 app → 任务结束/判 lost 后会话被唤醒；PR CI 失败 → 会话被唤醒；窗口到点 → 唤醒。

约束：task-template；合成 fixture；授权交付（ceiling: merged）。

Release-Urgency: immediate

## 实现与验收
- 后台任务和计划唤醒共用有界投递入口；失败仍 pending，五分钟以上记录告警并立即尝试。
- 死亡进程没有退出收据时按 lost 结算；输出文件不代表成功。
- PR 全绿仅作为接续条件，不作为合并完成；冲突与退队后仍允许后续观测。
- 无 executor 的 pending 状态不得返回 Running；审批、真实问题、停止及最终回答仍依事实优先。
- Harness：Spec、Compatibility（旧库/跨进程）、Observation（逾期告警）、Release（合并与验收交接）、AI Collaboration。
- 真机验收由编排方发版安装后完成；本任务交付止于合并，not live，不发版。
