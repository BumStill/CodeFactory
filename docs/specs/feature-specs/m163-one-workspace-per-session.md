# M163（高）：系统唤醒不得为同一任务新建 objective / 工作区

## 证据（v1.91.26，10-23 会话 c23d0d2d，M160 派单）
- 09:09:52 建 objective 20620bcb + 工作区 be2cfaa6；之后 09:18、09:21、09:22 多次 `system_wake_continued`，09:23:06 / 09:24:52 / 09:25:12 / 09:25:25 又各新建一个 objective（f9962cad/2fb2c736/acb11581/444e10b7）与新工作区（9a667c54/5d053722/3164c4f0/38b331e7）。
- 会话 09:25 报告：「当前执行工作区是 38b331e7…不是原 M160 工作区 be2cfaa6…此前跨工作区编辑已被工具拒绝」→ 已写的代码不可达，ended「任务已结束」，未开 PR。
- 派单时 dispatch 返回 `session … did not start a new turn in time`（首回合启动慢），可能与重复准入有关。
## 需求
- R1 一个会话的同一任务（同一授权目标）只有一个 objective 与一个托管工作区；系统唤醒 / 用户「继续」/ 重启续跑都必须重新绑定**已有**工作区，不得新建。只有用户明确开启新任务（新建会话或显式新目标）才新建。
- R2 已发生的分裂可修复：启动时检测同一会话多个 active 工作区，保留有改动/提交的那个为当前（多个都有改动时合并到最早的并保全其余到 rescue ref），其余转 closed，不丢改动。
- R3 dispatch 首回合「did not start a new turn in time」：查明是否导致重复准入；超时返回不得引发第二次准入。
- R4 测试（先失败）：合成 fixture 连续 3 次系统唤醒 → 断言 objective 与工作区数量仍为 1、代码改动可见；分裂修复路径；dispatch 慢启动不重复准入。
## 约束
- task-template；合成 fixture；授权交付（ceiling: merged）。

Release-Urgency: immediate

## Implementation Notes
- Spec、Compatibility、Observation、Payload、AI Collaboration Harness：以合成 SQLite 和真实 Git worktree 回归验证身份、文件内容、恢复幂等性；不修改用户数据库。
- 不发版；真机验收由编排方发版安装后完成：同任务连续唤醒、重启及继续，核对 objective/workspace 身份、改动和派单次数。
