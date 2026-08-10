# 持久交付恢复：业务设计

## 问题

1. 正式版交付等待在 PR 已合并、版本已发布后仍只写心跳，重启后没有执行者接管。
2. required check 可修复失败被终态回交，用户必须再次催促。
3. 工作树仍有未提交实现且没有 PR 时，交互回合仍可被标记为 completed。

共同原因是系统把进程内 AgentLoop 当成长任务 owner，又把 UI 活动快照当作权威状态。

## 用户价值

- 一次授权覆盖同一 objective/repo/ceiling 内的正常恢复，不重复询问。
- 用户看到“系统正在等什么、谁能改变状态、最后实质进展何时发生”。
- 完成只代表真实达到用户要求的本地/CI/merge/release/live 边界。
- 人的注意力只用于关键业务选择，不用于推动、重试或排查系统执行。
- 用户明确要求完成或离开时，系统自动执行推荐方案，不等待普通业务确认。

## 成功指标

- 可恢复阻断用户回交率降到 0。
- 同一 change-set 重复 PR 为 0。
- 未达 requested ceiling 的业务 completed 为 0。
- 强制结束并重启 App 后，30 秒内接管非终态交付。
- 非业务阻断用户回交率为 0%。
- 无人值守 objective 的推荐配置等待次数为 0；核心输入请求同一 objective 最多一次。

## 非目标

- 不放宽 required checks、review、签名、release 或 live verifier。
- 不使用 admin bypass、force push 或重复 PR 作为恢复手段。
- 不自动猜测旧历史记录的 repo/change-set/PR 身份。
