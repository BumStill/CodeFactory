# M205：到期的收尾唤醒从不触发（M202 真机验收未过）

Release-Urgency: immediate

## 真机证据（v1.91.61，22:44 重启后观察 18 分钟）
M202 的「等待都绑定可查事实」已生效（wake_source 都是 `pending wakeup：<id>`），但「到期必须投递」未生效：
| 会话 | 唤醒 id | next_at | fire_count |
|---|---|---|---|
| 019a75d8 | thin-closeout-019a75d8-…-857 | 18:28:44 | 0 |
| 0a80c2e4 | thin-closeout-0a80c2e4-…-123 | 18:28:44 | 0 |
| 2e267368 | thin-closeout-2e267368-…-897 | 18:28:44 | 0 |
| 22e55d80 | thin-repair-99d1d5af / thin-repair-44cda938 | 22:35 / 22:36 | 0 |

`session_audit_events` 里 019a75d8 只有 M199 时期的 `thin_wait_guard closeout_wake / m199_closeout_wake`（self_continue_fires 0），之后没有任何投递或拒绝记录——过期 4.5 小时、跨一次重启都没人碰它。PR #927 的 R3 测试 `m202_overdue_wake_is_audited_and_delivered` 通过，说明测试夹具和真实路径不一致。

## 要求
1. 先用上表这几个会话的真实形状（匿名化：waiting_wake + 已合并 PR + closeout/repair pending 唤醒，next_at 远早于启动时间）造夹具，复现「不投递、不审计」，先红。
2. 找出真实调度器为什么跳过它们（只扫某种前缀？只扫启动后创建的？被 thin 状态 guard 静默过滤？），写清根因。
3. 到期唤醒要么投递（fire_count+1、写审计），要么在审计里写明拒绝原因并把会话结算到明确终态（比如收尾完毕 → ended + 人能看懂的原因）；不允许「pending 且永远不动」。
4. 全局不变式测试：任意时刻 pending 且过期超过 2 分钟的唤醒数为 0（扫描周期内）。
5. 启动后一个扫描周期内处理掉所有积压过期唤醒。

## 验收
装机后 10 分钟内：`wakeup_schedules` 里 status=pending 且 next_at < now-600 的条数为 0；上表 3 个收尾会话进入 ended 且原因可读。

## Implementation Notes
- R1–R5 对应上文要求 1–5，原验收含义不变。合成 fixture 不使用真实会话 ID、生产消息或生产数据库。
- Harness：Spec、Compatibility、Observation、Release、AI Collaboration；无 UI 改动，不触发 Viewport。按执行模板单执行流，不调用 delegate_tasks。
- 已定位两处独立风险：生产 start_runtime 在首次 drain 前串行等待全部 waiting_wake 对账（包含外部观测）；drain 又在审计前用工作区恢复预算静默过滤 attempts>=3 / settled_reason 的行。旧测试直接调用 drain + 成功 Recorder，既不执行启动顺序，也不覆盖耗尽预算。
- 本任务只交付合并；真机安装与应用验收由编排方另行发版后执行，not live。
