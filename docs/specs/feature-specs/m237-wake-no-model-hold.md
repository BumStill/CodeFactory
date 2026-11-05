# M237：没有可用模型时暂缓系统唤醒

## 问题
PR 合并时所有模型都不可用，收尾唤醒每分钟写入一条 user 消息。真机上 `6c6dd8a2` 写入 93 条，`6e4e311b` 触发 77 次。这些消息会在模型恢复后进入上下文，占用每日预算。

## 根因
唤醒扫描不看模型是否可用，到点就写消息。回合因没有模型而结束后，`model-availability-*` 唤醒和未回答的消息都让 `delivery_closeout_may_end` 返回 false，「触发 2 次后结束」走不到，`ensure_terminal_wake` 于是每 60 秒重排一次。

## 要求
- R1：会话有未到点的 `model-availability-*` 唤醒时，其它到点唤醒不写消息、不起回合、不计 `fire_count`，`next_at` 顺延到模型唤醒的时间。
- R2：模型唤醒到点时，顺延的唤醒与它一起投递，合并为一个回合。
- R3：收尾和修复唤醒触发 2 次后，每次重排按指数退避，最长 1 小时。这里只限制频率，不结束会话：`may_end` 为 false 可能表示会话还有当前任务。

## 覆盖范围
R1 放在唤醒扫描入口，所以 `thin-closeout`、`thin-repair`、`thin-continue`、`thin-source-repair` 和 agent 自己排的唤醒都受约束，不按来源逐个处理。

## 不做
- 不删除已写入的重复消息，历史不改。
- 不在发给模型时去重。受影响的两个会话已结束，修复后不再产生新的重复。

## 验收
- 单测：`m237_due_wakes_hold_until_the_model_wake_without_writing_messages`、`m237_closeout_rearm_backs_off_after_two_fires`。
- 真机：额度封顶期间有 PR 合并，10 分钟内该会话 `fire_count` 不再增长，没有新增「定时唤醒」消息；预算恢复后自动收尾。
