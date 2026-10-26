# M184：唤醒被分裂工作区恢复挡住并热循环

## 需求（任务原文）
- R1 唤醒投递不得被「分裂工作区恢复」失败阻断：恢复失败（目录不存在/非 git 仓库/锁被占）时，按会话实际状态决定下一步——PR 已合并/已关闭 → 直接结算会话（ended，记录收尾已无需工作区或收尾由系统完成）；仍需工作区 → 从登记分支/远端重建（M151），重建失败 → waiting_user 给人话原因。绝不无限 defer。
- R2 退避与去重：同一会话的恢复失败按指数退避（≥1 分钟起），同一错误不得每秒重试；每会话最多 N 次后转为可见原因。加 CPU/频率不变量测试。
- R3 repo 锁：持有者进程已退出（pid 不存在或进程实例不匹配）时立即回收，不等 5 分钟过期；app 启动时回收上一实例的锁。
- R4 测试（先失败，跨进程）：PR 已合并且工作区目录已删除的 waiting_wake 会话 → 启动后 ≤1 分钟 ended；锁由已退出进程持有 → 立即回收；恢复失败不热循环（日志频率断言）。
- R5 真机验收：装新版后编排方列出的 14 个存量会话全部结算，wakeup_schedules 无过期 pending。

Release-Urgency: immediate

## Implementation Notes

- 合成 fixture，不操作生产会话。按 task-template §8，由编排方发版安装后验收 R5；本次仅授权交付至合并，不发版。
- Applicable Harnesses：Spec、Compatibility（持久恢复与进程身份）、Observation（有界重试）、Release（合并收尾）、AI Collaboration。
- 单执行流：任务模板明确禁止 delegate_tasks，不启动该工具。
- 已定位：chat 准入虽吞掉 split recovery 的错误，但 split 绑定未修复；cleanup supervisor 同时反复进入恢复。repo 锁仅检查 TTL，没有复用已存在的 process_identity_is_live。
