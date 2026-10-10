# 「停止」真的停下，插话回报如实（CF-STOP）

## 背景与用户决策

- **M47**（2026-10-09 17:12）：用菜单「停止当前执行」停掉会话 fc55d10b，objective 变成 cancelled。17:25，这个会话的交付记录在 CI 变绿后照样自动 squash 合并了 #584。用户按了停止，十几分钟后东西却被合进了主干。
- **M63**（2026-10-10 19:3x，v1.84.0）：会话当前这一轮卡在交付工具里等锁时，入口 `send(mode=steer)` 回报 `delivered / current_run`，但模型要等下一次调用才读得到插话，而那一刻不会来。停止之后，这条插话和后面排队的消息先后都送了进去，同一条消息落了两遍。

用户决策：停止之后，这个会话名下**还没发生**的交付动作一律不再执行；已经发生的动作不回滚，且会话里要如实说明现在停在哪。

### Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| CF-STOP-R1 | 用户或编排方停止一个会话后，这个会话名下所有还没发生的交付动作（等 CI 后自动合并、推送、开 PR、发版）都不再执行；已经发生的不回滚。会话里如实说明「已停止，PR #N 保持打开，没有合并」 | 集成测试：停止后 CI 变绿，PR 不被合并；文案 |
| CF-STOP-R2 | 如果交付走的是 GitHub 自动合并（auto-merge），停止时一并关掉这个 PR 的自动合并 | 测试：用合成的 GitHub 客户端断言关闭调用 |
| CF-STOP-R3 | 停止之后想继续，需要用户或编排方明确发起；不会因为后台恢复回路而悄悄恢复交付 | 测试 |
| CF-STOP-R4 | 入口的 `send`：当前这一轮卡在工具调用里、插话一时读不到时，回报 `queued` 或明确写「当前工具返回后读取」，不能报 `delivered`；同一条消息不能因为停止后又续跑而送达两次（幂等） | 集成测试：复现 M63 的时序，断言回报和落库次数 |

### Applicable Harnesses

Spec Harness；Compatibility Harness（已有的交付记录）；Observation Harness；Release Harness（不能误发版）；AI Collaboration Harness。

### 测试矩阵

- 停止发生在等 CI、正在推送、已开 PR 待合并这三个阶段。
- 停止后 CI 变绿。
- 插话：当前一轮正常、卡在工具里、刚停止三种情况；同一条消息发两次。

## Implementation Notes

本节只记录实现所依赖的技术约束，不改变任何需求或验收标准。

- 停止事实由一个**会话级持久栅栏**（`delivery_stop_fences`，按 `session_id` 定位）承载：停止时写入，交付阶梯在每一个"还没发生"的动作前读一次；读得到就停在那一步，并把"PR #N 保持打开，没有合并"写成结论。
- 栅栏只有**用户或编排方的显式继续**才会清除（`resume_delivery`）。后台恢复回路（`plan_startup_recovery`）只读栅栏，不清除。
- PR 侧的收尾（清 `merge-queue: armed` 标签、`gh pr merge --disable-auto`）是幂等的外部动作：任一失败都不回滚已经发生的事，只在结论里如实写明哪一步没能完成。
- 入口 `send` 的 `steer` 只有拿到"这一轮真的读到了这条插话"的证据（`steer_applied`）才回报 `delivered/current_run`；有界时间内拿不到就回报 `queued`（`ahead: current_run`，说明"当前工具返回后读取"），不再猜。
- 同一条消息带一个客户端消息 id；插话队列与后续续跑都按 id 判重，重复的同一条只落一次。
