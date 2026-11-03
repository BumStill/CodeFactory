# M209：重新打开会话的定时唤醒必须推进

Release-Urgency: immediate

## 原始任务规格（逐字保留）

## 证据（v1.91.64，06:04:38 启动）
M206 正确地把三个被误结束的会话（45191579、beafe825、5ee1876f）重开成 waiting_wake「系统自动重试/自愈：接续中断回合」，但：
| 会话 | 唤醒 id | status | next_at | fire_count |
|---|---|---|---|---|
| beafe825 | thin-source-repair-beafe825-… | pending | 06:05:42 | 1（01:42 那次） |
| 5ee1876f | 同前缀 | pending | 06:05:42 | 1 |
| 45191579 | 同前缀 | pending | 06:05:43 | 0 |
到 06:45 三个会话安装后消息数均为 0，唤醒一直 pending。安装器自己的验证会话也以「定时唤醒未开始回合（上一回合已结算）」结束。06:45 我用 dispatch send 手动推进，三个立即 started——说明会话本身可运行，是唤醒投递路径没走通。

M205 的「到期唤醒必须投递或结算」不变式没有覆盖「同一 id 被重置回 pending（fire_count>0）」以及「投递了但回合没开始」这两种形态。

## 要求
1. 夹具复现：(a) 复用 id、fire_count=1、重新置 pending 且已过期；(b) 唤醒投递时判断「上一回合已结算」而不开新回合。先红。
2. 根因写清：调度器是否按 id/fire_count 去重？「已结算」判断是否把重开后的会话当成已完成？
3. 不变式扩展：投递 = 必须产生新回合或写明拒绝原因并结算；pending 过期 >2 分钟数为 0，与 fire_count 无关。
4. 重开会话时新建唤醒 id（或显式重置调度状态），不复用旧的已触发记录。

## 验收
装机后 10 分钟内不存在 pending 且过期 >10 分钟的唤醒；人为造一个误结束会话经 M206 重开后 2 分钟内出现 assistant 回合。

## 补充（v1.91.65，07:10 启动后 50 分钟）
`thin-source-repair-*` 过期 pending 共 17 条（多数 fire_count 0，跨 15 个会话），不止重开会话。45191579、5ee1876f 因此在重启后进入 waiting_user「系统自动重试 2 次后仍未推进，请决定换个做法继续还是结束」——实际一次都没投递。范围扩大到：所有 thin-source-repair 唤醒。

## Implementation Notes
- R1–R4 对应要求 1–4；所有夹具仅用 synthetic 数据。
- Harness：Spec、Compatibility、Observation、Release、AI Collaboration；无布局改动，不触发 Viewport。
- 按任务模板单执行流，不调用 delegate_tasks；本次交付上限为合并，真机验收由编排方另行发版安装后完成（not live）。
- 真机核对：启动后扫描 pending 且 next_at < now-600；重新打开合成误结束会话，确认两分钟内有 assistant 回合；核对拒绝审计与重试预算，不把未准入计为成功执行。
