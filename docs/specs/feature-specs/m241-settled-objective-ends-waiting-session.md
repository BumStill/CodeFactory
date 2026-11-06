# M241：已结算目标结束等待会话

## 背景与用户决定
PR 已合并、objective 已结算，但会话仍停在 `waiting_wake`，迟到的修复唤醒反复被拒绝。拒绝本身正确，缺少的是取消残余唤醒与结算会话。授权交付到合并队列；发版与真实安装版验收由编排方另行执行。

## 规格（任务原文）
- R1：系统唤醒到点时，如果会话没有未结算的 objective，不起回合、不写 user 消息、不重试，并取消该会话其余待触发的系统唤醒。
- R2：同一处理里，把会话结算为 `ended`，`reason` 写明真实原因。不留等待中间态。
- R3：`objective.rs` 里「迟到唤醒不是授权」的约束保持不变：仍然不新建 Objective、不新建工作区。
- R4：用户消息不受影响。结算后用户发新消息，走正常入口。
- R5：只有「确实没有未结算 objective」才走这条路径。查询失败、数据库忙这类错误不算，不能因此结束一个还在做的会话，要走现有的有界重试。
- R6：排查所有会被这条拒绝影响的唤醒来源（repair、closeout、continue、model-availability 等），把每种的行为写进规格。这属于恢复回路家族，不要只修 `thin-repair`。

## Requirements Traceability / 测试矩阵
| Req ID | 最低证据 |
| --- | --- |
| R1、R2、R3 | 真实 SQLite，completed/cancelled + waiting_wake + 到点与未来唤醒：扫描后 ended、全部取消、sink 未调用、user 消息为 0、目标/工作区行数不变 |
| R4 | 结算后通过正常 User 目标准入创建新目标，不复用已结算目标 |
| R5 | 未结算目标照常投递；查询出错不得结算，保持 pending 与有界重试 |
| R6 | 所有唤醒来源共用确定性结算边界，并保留目标准入的迟到唤醒拒绝 |

## Applicable Harnesses
Spec Harness：R1–R6 测试映射；Compatibility Harness：既有数据库、模型恢复与用户准入；Observation Harness：真实原因持久化；Release Harness：PR/CI/队列交付，not live；AI Collaboration Harness：单执行流，任务模板禁止 delegate_tasks，记录假设与复核结果。

## Implementation Notes
- 修复适用于 `thin-repair`、`thin-closeout`、`thin-source-repair`、continue、普通 schedule、`model-availability` 及后台任务完成通知；不能用字符串报错代替数据库事实。
- 模型恢复可能关联已 completed/legacy_orphan 的真实未回答用户任务，现有目标准入允许这种恢复；实现须沿用这一事实，不得误结算。未合并交付仍拥有目标时同理。
- 在同一数据库写事务中确认目标事实、写 ended、取消本会话所有 pending schedules；失败则回滚。没有目标表的极简/历史测试库不视为确定性无任务事实。
- 不修改用户消息入口，不扩大系统唤醒创建目标或工作区的权限，不改变已批准规格。

## 验收（真机，编排方发版后做）
下一单任务合并后，会话自动进入 ended，`wakeup_schedules` 里没有残留 pending，全程没有人工 `stop`。安装发布版本后核对真实主路径、会话审计与 SQLite；会话内以真实 DB 集成测试验收，不替代安装版验证。
