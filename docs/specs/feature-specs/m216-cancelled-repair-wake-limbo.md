# M216：PR 检查失败后的修复唤醒被取消，会话悬空

## 原始任务规格（逐字保留）

## 现场（2026-10-31，会话 ed737422 / PR #948）
- 13:08 会话推送后挂上「系统 PR 监视订阅：…/PR#948」，进入 waiting_wake。
- 13:24 CI 结束：check-rust 失败（真实测试失败）。
- 系统随后建了修复唤醒 thin-repair-41bfa6e9（13:25，「PR #948 检查失败：check-rust」）和 thin-repair-97dc1ff0（13:28，「被合并队列退回」）。两条都是 status=cancelled，fire_count=0，也没有新的替代唤醒。
- 结果：thin_session_state 一直是 waiting_wake，wake_source 仍写着那个 PR 订阅，可订阅早就触发过了。会话约 1.5 小时没有任何模型调用，直到编排方手动发消息。用户界面上看起来是「在等」，其实没人会叫醒它（违反「不许有悬空中间态」）。

## 要求
- R1：先查清楚是谁、为什么取消了这两条修复唤醒（合并队列打 blocked 标签？同一 PR 的新事件去重？M209 的过期唤醒清理？）。把根因写进规格。
- R2：不变量是，会话处于 waiting_wake 时，必须至少有一个真实会触发的唤醒源：pending 的 wakeup_schedule、正在运行的后台任务，或者尚未触发的 PR 订阅。取消唤醒的代码路径必须在同一事务里补上替代唤醒，或者把会话转成明确终态，或者转成 waiting_user（问题要写清楚）。
- R3：在后台加一个巡检，发现违反 R2 的会话就自动补一条修复唤醒，并记一条审计事件。不得删除对话，也不得往对话里写内部词汇。
- R4：测试要复现本次时序：PR 检查失败，紧接着合并队列打 blocked 标签，两条修复唤醒先后被创建。断言最终至少有一条唤醒会真正触发，会话会进入修复回合。

## 验收
- 本机复现同样时序后，会话在 2 分钟内开始修复回合，全程不需要人工发消息。
- health-watch 里「waiting_wake 且无唤醒源」的计数不再增加。

Release-Urgency: immediate

## Implementation Notes：根因（R1）

只读核对现场调度行与审计元数据，没有读取或复制对话内容。两条指定唤醒的 `fire_count=0`；同一时间窗的 `wakeup_rejected` 审计明确记录 `attempts=3`、`唤醒投递失败；系统自动重试/自愈`。第一条与更早的检查失败唤醒一同被拒绝，blocked 事件生成的第二条随后被同一预算拒绝。因此取消者是 `background_tasks::reject_exhausted_wakes`：`drain_due_wakeups_at` 把会话准入/投递失败计入 `workspace_recovery_budget`，第三次失败后取消全部 pending 唤醒。并非 blocked 标签本身取消，也不是 M209 过期去重。现场审计的检查名为 `scenario-gate-pr`，与任务描述的 `check-rust` 不同；修复不依赖检查名。

`reject_exhausted_wakes` 原来在事务中写 ended，但后续 `thin_derive::derive_state` 遇到未消费的外部失败，会重新派生 waiting_wake，仅写“持久投递器自动重试”文本，没有持久计划。`wake_source_cover` 又把所有非合并/非关闭的 completed_reason 计为来源：已经触发的失败/blocked PR 观测被误算成仍可触发的订阅，M197 巡检早退，形成悬空等待。

## Requirements Traceability 与测试矩阵

| Req ID | 实现边界 | 最低证据 |
| --- | --- | --- |
| R1 | 以上现场元数据 + 调度拒绝/派生/覆盖判定调用链 | 不复制真实对话的根因记录 |
| R2 | 已触发订阅不再覆盖；取消操作同事务保护等待；技术投递失败不消耗工作区恢复预算 | SQLite 事务测试、瞬态准入失败后成功投递、停止不复活 |
| R3 | 本地短周期巡检补 pending 修复计划与审计，独立于 ≥180s GH 观测 | 无来源遗留状态的幂等修复与审计断言 |
| R4 | 合成 CI failure → blocked 双事件，失败准入后由系统自动重试/自愈 | 真实持久调度器 → WakeSink 投递成功，fire_count 递增，对话写入数量保持零 |

## Applicable Harnesses

- Spec Harness：原始 R1–R4 与验收不变，以上表格映射测试。
- Compatibility Harness：不改既有 schema，对已取消遗留计划与旧观测补建新 ID；不复活停止会话。
- Observation Harness：持久审计、无来源计数与调度投递字段断言。
- Release Harness：本任务只交付合并；保留 immediate trailer，不执行发版。
- AI Collaboration Harness：限定 background_tasks/thin_derive/取消调用范围的独立只读审查；假设技术准入失败可自动重试，不代表用户要求停止；审查与验证结果见 PR。

## 真实应用验收分工

本会话执行合成 SQLite 集成测试与持久调度器验收，不声称已安装版 live。编排方发版安装后：制造 PR 检查失败，随后 blocked 标签；观察两分钟内自动进入修复回合；核对 pending/done 调度、fire_count、无来源巡检审计与 health-watch 无来源计数；确认用户停止后不再自动唤醒。
