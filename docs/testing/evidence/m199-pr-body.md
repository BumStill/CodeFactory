## 问题与实现
修复 M199：已合并 PR 的 cancelled 收尾唤醒旁边仍被派生出只有文案的 waiting_wake。

- R1：终端 PR 的派生查询真实 pending 行；缺失时在同一 SQLite 写事务补建唤醒及投影，wake_source 含行 id，refresh 不再二次覆盖事务投影。
- R2：当前 PR 前缀匹配的收尾 fire_count 累计最多 2；耗尽或工作区已清理直接 ended，保留现有文件与记录，不询问是否继续。成功收尾回答仍走既有终止事实。
- R3：write_state_and_cancel 仅 ended 取消唤醒，提问不取消 pending；原有停止、活执行者和未推送数据保护不变。
- R4：启动/180 秒周期共用对账，持久补建/结算写 thin_wait_guard 审计；不修改生产数据。
- R5：合成 fixture 覆盖 cancelled、刷新即补源、双次对账幂等、2 次耗尽、工作区消失、提问取消边界；append-only 登记 HLT-004 映射。

## Req ↔ 验收（执行结果持续补齐）
| Req | 测试 | 先红证据 | 验收 |
| --- | --- | --- | --- |
| R1/R4/R5 | m199_cancelled_closeout_reconcile_is_fact_backed_and_idempotent | pending 实际 0，期望 1 | 已转绿；双次对账 id 不变 |
| R2 | m199_exhausted_or_removed_workspace_ends_without_question | 实际 WaitingWake，期望 Ended | 已转绿 |
| R3 | m199_question_does_not_cancel_pending_closeout | pending 实际 0，期望 1 | 已转绿 |
| 兼容保护 | agent::thin_derive 全量，含停止/活执行者/脏工作区/跨进程/成功收尾 | 发现旧提问取消断言与 R3 冲突 | 旧断言仅改为保留 pending，不削弱文件保护；限流回归修复后复跑 |

## 等待入口、事实来源与边界
本任务涉及 thin_derive 的全部 waiting_wake 派生入口：
- 未知执行者：进程注册表未知，周期活性扫描接管；不是收尾凭据，扫描后重派生。
- 活后台任务：PID + process_started_at 核对；进程退出后由持久任务结果投递。
- 已合并/已关闭 PR：pending thin-closeout/thin-repair 的具体行 id；同 PR fire_count 上限 2；已完成回答或无需收尾直接结束。
- 检查失败/全绿通知：thin-repair pending 行与外部观测；既有单次去重，更新的外部事实触发下一轮。
- 未完成交付引用：thin_delivery_refs + thin_external_observations，系统有界心跳 ≥180s，受 GitHub 配额限制，终端后改收尾源。
- future/due wake：wakeup_schedules pending 行、next_at；一次或重复节奏由持久记录决定。
- 后台结果待投递：background_tasks.wake_state=pending；成功投递后推进。
- 旧等待/中断回合兜底：文案仅用于选择修复策略，不是已覆盖证明；wake_source_cover 查询任务、pending 唤醒、未完成外部观测；thin-source-repair 累计最多 2。
- executor_missing_state：refresh 后调用修复来源入口，同样最多 2 次自动补源。
取消入口 write_state_and_cancel：只允许 ended 取消；waiting_user 不再误杀收尾。

## Harness / AI Collaboration
Spec：原任务逐字落库 docs/specs/feature-specs/m199-closeout-wait-phantom-source.md，追加 Implementation Notes。
Compatibility：旧库 cancelled/queued=1、跨进程 fixture；无迁移。
Observation：pending id、状态、fire_count、双次对账与审计。
Release：只交付到 merged，Release-Urgency trailer 留待独立发版。
AI Collaboration：context scope 为 thin 派生、唤醒、合成验收；assumptions 为已合并事实来自持久观测而非文案猜测；review point 为独立只读来源/QA 审查及 CI；validation result 见测试结果。无 UI 布局变更，不触发 Viewport。

## 真实应用验收分工
not live。真机验收由编排方发版后完成：安装新版本，启动核对历史已合并会话，在一个 180s 周期后查询 waiting_wake 是否有 pending 收尾 id 或已 ended，核对 thin_wait_guard 审计；再过一周期核对幂等与最多 2 次。会话内只使用合成数据，不读写生产会话库。

Spec Amendment: 无。

Scenario-Test: HLT-001, HLT-004, RTE-005
README-Update: reviewed
README-Update-Reason: 内部持久收尾状态修复，无新增公共能力、安装、隐私或平台承诺。

Release-Urgency: immediate
