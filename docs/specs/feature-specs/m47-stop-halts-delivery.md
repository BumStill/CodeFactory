# M47：停止必须终止交付外部写入

## 已批准任务（原文）

## 现象（10-09 真机，需先确认是否仍存在）
- 17:12 用菜单「停止当前执行」停掉会话 fc55d10b（objective → cancelled）；17:25 它的交付记录在 CI 变绿后仍自动 squash 合并了 #584。
- 之后交付链路已有多次改动（CF-CLOSEOUT、合并队列 + `merge-queue: armed` 标签、M87 explicit_cancel 归因、M91 交付锁）。现在合并由合并队列执行：会话在 CI 绿后打 armed 标签，队列合并。停止后若标签已打上或交付记录仍会在 CI 绿后补打标签，问题仍在。

## 需求
- R0 先用合成 fixture 复现（或用代码+数据证明已不可能），把结论写进规格。若已修复：补回归测试锁住并说明由哪个改动修复，然后交付测试即可。
- R1 停止（显式取消）后：交付记录进入终止态，不再推送、开 PR、打 `merge-queue: armed`、发起合并或 auto-merge。
- R2 停止时 PR 已经 armed：移除自己打的 armed 标签（这是撤回动作，允许执行），并在会话里如实说明「已撤回合并排队；PR 保留未合并」。PR 本身不关闭、分支不删。
- R3 停止前已完成的外部写入（已推送、已开 PR）在会话终态里如实列出，不能说成「什么都没发生」。
- R4 用户再次发消息续做时，可重新授权交付；停止不是永久禁止。
- R5 测试：停止发生在 (a) 推送前 (b) PR 已开、CI 未绿 (c) 已 armed 未合并，三种时序各一条先失败的测试。

## 约束
- 只对「用户/编排方显式停止」生效；系统内部的恢复、重启接管不能被当成停止（参见 M87 的 explicit_cancel 误判教训）。
- 合成 fixture，不写生产 DB。

## Requirements Traceability

| Req ID | 实现与最低证据 |
| --- | --- |
| R0 | 合成裸 Git origin + StubRemote；`m47_stop_on_green_ci_never_arms_the_queue` 修复前失败（queue_arms=1，期望 0），修复后通过 |
| R1 | CI 返回后及每次排队重试前检查栅栏；停止持久化终止交付；推送前与 CI 等待回归 |
| R2 | 在途排队请求返回后补偿撤回；已 armed 的停止报告必须包含撤回结果，失败不伪报成功 |
| R3 | 保留已完成 steps 与 PR 身份，报告已推送/已开 PR，不回滚分支 |
| R4 | 新用户消息明确重新授权交付时清除会话栅栏；恢复/重试不清除 |
| R5 | 三时序合成测试，已有行为若已修复则保留现有回归并记录历史修复 |

## Applicable Harnesses

Spec Harness；Compatibility Harness（无 DB schema 变更）；Observation Harness（停止报告与已完成写入）；Release Harness（只合并不发版）；AI Collaboration Harness。

## R0 结论与 Implementation Notes

- 原有 CF-STOP 已覆盖推送前、CI 等待中、PR 撤回，但 M77 引入的 CI 成功后排队路径位于最后停止检查之前，确实仍可停止后打标。
- 合成复现：CI adapter 返回 Success 的同时设置显式停止，原实现 `queue_arms=1`；不访问生产数据库或 GitHub。
- 排队请求已经在途时不能撤销传输；返回后必须撤回本次标签。已在远端完成的合并不能声称未合并，也不回滚。
- 不修改队列、场景注册表、工作流或治理 validator。
- 真机验收由编排方发版安装后完成：分别在推送前、CI 未绿、已 armed 未合并时通过菜单或 dispatch stop 停止；核对 delivery 终态、标签移除、PR/分支保留、会话说明；再发送明确交付授权，确认可重新交付。系统内部恢复不得制造 user_stop。
