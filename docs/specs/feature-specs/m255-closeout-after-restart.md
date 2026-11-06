# M255：升级重启后，PR 合并了却收不了尾，系统唤醒空转到放弃

## 规格（任务原文）
- R1：先用数据定位。查三个会话的 `objective_events`、`wakeup_schedules`、`thin_external_observations`、`chat_turn_state` 时间线，写清每一步是谁触发的。不要只读代码推断。
- R2：重启不得退休还在等 PR 的 objective。查清 `retire_legacy_objective_on_thin_session` 的调用路径，在等 PR 的会话上不再退休。已退休的存量会话要能恢复（R3）。
- R3：PR 已合并时，会话能走到收尾，不依赖 objective 的状态。收尾完成后结束，状态写真实结果。收尾无法进行时结束，并写明 PR 已合并、哪些收尾没做。不许写成「任务未完成」。
- R4：合并后的系统唤醒有上限。同一事实重复触发却没有新进展时不再起模型回合，上限按真实触发次数算。模型回「已完成」且没有工具动作时算没有新进展。
- R5：列出所有会在合并后重新排期的唤醒来源（closeout、source-repair、repair、continue），写明每一种在「PR 已合并」时的行为。这是家族修复，不要只修其中一种。
- R6：M241、M251 的测试全部保留。`legacy_orphan` 不算任务完成。

## 测试（任务原文）
- 真实 DB：objective 已退休 + PR 已合并 + pending 唤醒 → 走到收尾并 ended，模型回合最多一次。
- 升级重启：等 PR 的会话，重启后 objective 不退休；PR 合并后正常收尾。
- 空转：收尾已完成后的迟到唤醒不起模型回合。
- 反例：PR 未合并时会话继续等待。

## 验收（任务原文）
PR 未合并时安装新版，PR 合并后会话自动收尾并 ended，全程没有人工 stop，收尾后没有新的模型调用。

## 实施记录
Applicable Harnesses：Spec、Compatibility、Observation、Release、AI Collaboration。真机验收由编排方发版后完成，本任务不发版。合成 SQLite 集成测试覆盖恢复、唤醒准入与终态投影；只读现场库用于定位，不修改生产会话。

Requirements Traceability：R1 对应下述现场时间线；R2 对应退休保护测试；R3/R4/R5 对应四类唤醒的合并后单次收尾测试；R6 对应保留 M241/M251 回归。

AI Collaboration：context scope 为任务原文、现场 SQLite 时间线及 objective/background_tasks/thin_derive；不假设退休等于完成；review point 为共享准入边界与合并事实作用域；validation result 随交付证据记录。按任务模板禁止 delegate_tasks，采用单执行流。

### R1 现场数据定位（只读查询，时间为数据库原始毫秒/秒）
- 73ddd1b2：objective_events 最后一次 `system_wake_continued` 为 1793975164613，执行者为 system_wake/chat；重启收敛把 71 个 chat_turn_state 写为 `orphaned_by_restart`，最后更新时间 1793976080518。objective 退休直接更新表、不写事件，因此退休发生不能从缺失的事件伪造 actor。wakeup_schedules 显示 continue 实际触发 5 次、source-repair 1 次、repair 2 次；后续 repair 多条 cancelled 且 fire_count=0，不能算实际触发。现场首次读取 thin_state 是 31 次准入失败后 ended，第二次读取已被系统投影回 waiting_wake；external observation 中 merged_at=1793980701000，而 ceiling=through_release、completed_reason=NULL，仍显示等合并。说明还有「合并事实被发版 ceiling 掩盖」的衔接缺口。
- cb4d9711：最后 system_wake_continued=1793975812762；24 个回合由重启写为 orphaned_by_restart（1793976080518）。continue 触发 3 次、source-repair 2 次、repair 3 次；合并时间 1793979556000，through_release 的 completed_reason 仍 NULL。最终为明确的用户 stop（thin_state.updated_at=1793980871419），不是成功收尾。
- db17572f：external observation 已记录 PR #1008 合并（1793969494000）。closeout 单行累计 fire_count=4、source-repair 累计 6、continue 1、model-availability 1；不是按行数计数。最后系统续接事件在 1793972455075，decision_applied cancelled=1793972925514，26 个 chat_turn_state 最终 cancelled，由编排方 stop 终止；无证据证明 §8 已执行。

### R5 来源行为约束
closeout（含 closeout-source）、source-repair、repair（含 repair-source）、continue 都必须在共享唤醒入口核对同一 PR 合并事实；合并前保持原等待/修复语义，合并后只准入一次收尾，不按来源各领一次额度。已有收尾工具证据与最终回答则结束；没有证据的纯文本「已完成」只能报告未核验的收尾项，不能宣称收尾成功。迟到来源全部取消，不新建 objective/工作区。
