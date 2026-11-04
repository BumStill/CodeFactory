# M229：会话「等 PR 合并」时，合并队列把 PR 移出了也不会被唤醒

## 现场（2026-11-04，同一天两次）
- 7b078d78（M223）/ PR #962：07:56 合并队列打上 `merge-queue: blocked` 标签，并评论「required check scenario-gate-pr failed」。会话一直挂着「PR 未合并前不结束会话」(waiting_wake)，没有被唤醒。07:58 由编排方手工告知。
- f6fdddd5（M226）/ PR #964：08:11 因 check-rust 失败被移出（Windows 上本 PR 新增的测试失败），会话同样挂在「PR 未合并前不结束会话」。08:14 由编排方手工告知。
- 合并队列被移出后不会再自动重试。会话只盯 merged/closed，所以只要 CI 红，就是永远等不到的等待（违反「等待必须有能改变状态的执行者」）。

## 要求
- R1：会话的 PR 监视把以下三种情况都当作唤醒事件，并在唤醒正文里写明哪个检查失败、失败的 run 链接或摘要：
  - PR 带上 `merge-queue: blocked` 标签，或出现 `<!-- merge-queue:blocked` 评论；
  - 当前 head 有必需检查的结论为 failure 或 cancelled；
  - PR 被关闭但没有合并。
- R2：唤醒后的回合按「修到 CI 全绿 → 移除 blocked、重新 armed」继续执行，不得把会话结束成「已交付」。
- R3：同一个 head 上同一个检查失败只唤醒一次（去重）。head 变化后重新计算。
- R4：测试覆盖：
  - 模拟 PR 状态从「armed、checks pending」变为「blocked 加 check failure」，会话在一个轮询周期内被唤醒，唤醒正文含失败检查名；
  - 再次轮询同一状态不重复唤醒。
- R5：遵守 GitHub 限流（轮询 ≥180 秒，与合并队列共用 5000/h 的额度）。gh 返回空结果不能当作「全绿」。

## 验收（真机）
- 在一个会话的 PR 上让必需检查失败（或等下一次自然发生），合并队列移出后几分钟内，会话自动开始修复，不需要人工发消息。

Release-Urgency: immediate

## Implementation Notes
- 沿用 `heartbeat_external`、共享 GitHub budget 和持久唤醒投递器；不新增 GUI 操作入口。
- 在 SQLite 持久记录 PR/head/失败检查事件，避免文案变化、重复轮询或重启重复唤醒；head 变化重新计数。
- Harness：Spec、Compatibility（旧库增量建表）、Observation（失败详情）、Release（合并交接，真机由编排方发版后验收）、AI Collaboration。
- 测试矩阵：R1/R4 blocked+failure、comment-only、cancelled、closed；R2 修复提示与非终态；R3 相同失败及新 head；R5 空结果、180 秒 CAS 与共享 budget。
- 本任务按模板单执行流，不调用 delegate_tasks。真实应用验收由编排方发版安装后完成：等待 PR → 失败退队 → 几分钟内自动修复 → CI 全绿后移除 blocked 并 armed。
