# M163 实施与验收记录

## 证据（先红后绿）
- R1 先红：临时关闭准入复用分支后，`m163_three_unanchored_wakes_keep_one_objective` 在同一次系统唤醒上得到两个不同 objective id（left/right 不一致），即复现「同一任务被重复准入」。恢复修复后同测试通过。
- R1 绿灯覆盖：合成 SQLite + 真实 Git 仓库 + 真实分配器，连续三次无 transport anchor 的唤醒（每次换进程身份模拟重启）后断言 objective 与 workspace 各 1 个、工作目录不变、改动文件内容仍可读。
- R2：`m163_split_recovery_preserves_both_dirty_workspaces` 覆盖两份 dirty 内容合并、`refs/codefactory/rescue/<id>` 保全、多余记录关闭、重复恢复幂等。
- R3：`M163 slow admission retries share the original send` 使用真实 store，确认首次发送超时后重试只产生一次 `send_message`。现有 in-flight guard 已满足要求，未改派单产品代码；超时只结束等待，不取消原发送。
- 回归：`commands::chat::tests::m144_admission_retries_a_real_concurrent_writer`、`tests/no_window_regression.rs`、`pnpm test`、`pnpm build`、治理基线均通过。

## 实现要点
- 准入：系统唤醒且无回合锚点可解析时，回退到会话内未终结的 objective；若会话已无未完成任务则拒绝并为该唤醒创建新 objective，避免完成后的通知另开工作区。
- 恢复：启动清理与准入共用同一恢复路径；先做轻量计数门控，只有确实存在多个活动工作区才进入；所有目录写入前先落 rescue ref，身份不符或冲突时保留原状并稍后重试；恢复仅重写工作区文件并把索引 reset 回 HEAD，不改分支。
- 冷却期保护：清理循环已持全局分配锁，因此恢复拆分为「已持锁」和「自持锁」两个入口，避免非重入互斥锁自锁。
- 准入侧恢复是尽力而为：并发写锁导致失败时记录 warn 并延后，绝不阻断正常发送（M144 回归覆盖）。

## Harnesses / AI Collaboration
- Spec：`docs/specs/feature-specs/m163-one-workspace-per-session.md`（含需求原文与约束）。
- Compatibility：Objective 状态机既有语义不变；旧 schema（无 `requires_user_action` 等列）走窄更新分支。
- Observation / Payload：以真实文件内容、rescue ref 内容、objective/workspace 行数作断言，不以进程存活或空数组代替。
- AI Collaboration：context scope 为目标准入、工作区分配与恢复、派单慢启动；assumption 为系统消息不构成新授权；review point 为独立只读审查（准入根因、恢复安全性）；validation result 见上。
- 本任务无 UI 改动，无 viewport 要求；未发版，**not live**。真机验收由编排方发版安装后完成：同一任务连续系统唤醒、重启续跑与用户「继续」，核对审计中 objective/workspace 身份不变、代码改动可读、派单只发生一次；对已发生分裂的安装，启动后核对保留内容与 rescue refs。

README-Update: reviewed
README-Update-Reason: 内部准入与恢复修复，不改变公开安装、平台或隐私承诺。
