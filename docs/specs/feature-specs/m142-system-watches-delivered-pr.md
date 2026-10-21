# M142：已交付 PR 由系统心跳监视即算唤醒源，不再问用户「继续监视还是结束」

## 证据（v1.91.13，含 M140）
- README 会话 5a38a52d：交付 PR #813 并 armed 后未自己挂后台监视；thin 状态转为 waiting_user「外部事项尚无法确认完成，且没有活着的唤醒源。请选择继续监视或结束任务。」——用户需要介入。
- M140 已让心跳按 thin_delivery_refs 低频查询 PR 真实状态 → 系统本身就在监视。
## 需求
- R1 derive_state：会话存在未完成的已交付 PR（thin_delivery_refs，ceiling 未达）时，视为有唤醒源（系统 PR 监视），状态 waiting_wake（等待说明：「等 PR #N 合并/CI」），不转 waiting_user；PR 合并/达到 ceiling → 唤醒收尾；PR 关闭或检查失败 → 唤醒会话处理失败。
- R2 检查失败时唤醒会话并附失败检查名，模型修复后再交付（不问用户）。
- R3 waiting_user 只用于真正需要人的决定（审批、模型主动提问、无法自动判断）。
- R4 测试（先失败）：交付后不挂监视 → waiting_wake（系统监视）；PR 合并 → ended；检查失败 → running 并收到失败信息。
## 约束
- GitHub 调用遵守预算（≥180s）。授权交付（ceiling: merged）。不要超出规格自加范围。

## Implementation Notes
- 未指定规格文件名，原文保存于本文件。
- Harness：Spec、Compatibility、Observation、Release、AI Collaboration；无布局变更。
- R1/R3：交付引用作为系统心跳监视事实，但不覆盖停止、审批与真实提问。R2/R4：SQLite 合成 fixture 覆盖失败观测、持久唤醒投递、修复与最终收尾；既有 M140 合并回归保留。
- 心跳仍遵守持久 180 秒 CAS 和全局 GitHub 预算；无需 GUI，锁屏可运行。
- 真机验收由编排方发版后完成：交付 PR 不另挂后台任务时应显示等待 PR 合并/CI；制造失败检查后应自动运行并携带检查名；修复交付、合并并收尾后应结束；审批/真实问题仍等待用户。
- AI Collaboration：context scope 为 thin 推导及心跳观测；assumptions 为未知/查询失败不能作为完成事实；review point 为停止优先级、唤醒去重及预算；validation result 记录于 PR。本任务遵循模板单执行流，不调用 delegate_tasks。
