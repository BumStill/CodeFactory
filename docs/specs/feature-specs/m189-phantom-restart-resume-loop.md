# M189：幽灵重启恢复循环

## 原始任务规格（原文）

# M189（紧急）：没有重启，已完成的会话却每 ~10 分钟被「重启中断·自动继续」叫醒一次（无界空转）

## 本机证据（v1.91.50，10-27）
- 会话 987a523b（M188 交付会话，PR #906 已合并、收尾完成、回答「无需恢复执行」）。
- 应用进程自 09:47 起未重启，但 messages 里该会话从 08:27 到 18:04 每 10~11 分钟插入一条 system：「上一轮执行在 background_task_list 这一步被应用重启中断（结果未知），系统已自动继续…」，随后 3 次模型调用，再结束；24 小时内 37 次，6 小时 99 次模型调用。thin_session_state 一直是 running。
- 后果：烧额度；安装器把它当真实活动会话，v1.91.51 安装被挡 6 小时。
- 同族：恢复回路无界（见 recovery loop family，第 4 次复发）——按规则先做家族审查，而不是只补这一个入口。

## 需求
- R1 根因：找出非启动时刻把「已结束回合里最后一个工具调用」判为「被重启中断」的路径（疑似周期性 sweeper / wake 处理复用了启动恢复逻辑，或 background_task_list 工具行未落终态）。结论写进 PR。
- R2 不变量：「被应用重启中断」只能由**本进程启动之前**开始、且无终态的工具调用触发；同一工具调用最多触发一次恢复（幂等键落库）；回合已产生最终 assistant 回答的会话不得被自动续跑。
- R3 家族审查：列出所有会自动给会话追加消息/续跑的入口（启动恢复、wake、sweeper、完成门禁、交付恢复等），确认每个都有基于真实 claim 计数的硬上限与幂等，表格写入 docs/specs 相应规格。
- R4 回归测试（先失败，合成 fixture、跨进程）：完成回合后进程持续运行 ≥3 个 sweep 周期（可用缩短的测试间隔）→ 断言零新增 system 恢复消息、零模型调用、thin 状态为 ended/idle；再测真实重启只恢复一次。登记 HLT 场景为 PR hard gate。

## 约束
- task-template（harness + SDD + TDD）；合成 fixture；不改生产数据。授权交付（ceiling: merged）。

Release-Urgency: immediate

## Requirements Traceability

| Req | 实现边界 | 最低证据 |
| --- | --- | --- |
| R1 | 心跳到恢复入口、提示与去重键 | 根因调用链及失败回归 |
| R2 | 进程边界、终态过滤、持久 claim | 已完成/同进程/重启/重复 claim 断言 |
| R3 | 自动消息与恢复家族 | 逐入口审查表，缺口如实记录 |
| R4 | 合成 SQLite、真实子进程与三次 sweep | PR hard gate 绑定测试 |

## Applicable Harnesses

Spec、Compatibility（历史 SQLite）、Observation（恢复消息与调用次数）、Release（仅交付至合并，另行发版）、AI Collaboration（单执行流）。无 UI 布局修改，不触发 Viewport。

## Implementation Notes

根因：`objective_supervisor` 每 12 次轮询调用 `resume_running_without_executor(Heartbeat)`，复用 `resume_thin_running_session`。候选 SQL 接受 `control.status<>'cancelled'`，包括 completed；启动候选只检查 running 投影；提示查询无条件取最后工具（包括已完成工具）；去重键使用 `turn.updated_at`，每次续跑都会变化。因此十分钟宽限不是总预算。

## R3 恢复家族审查

| 自动入口 | 幂等 / 实际 claim 计数 | 硬上限与结论 |
| --- | --- | --- |
| 启动恢复、心跳 running 扫描 | 旧进程 active 快照；工具 ID claim，根回合唯一索引 | 本次新增每根回合最多 1 个 claim；最终回答与终态禁止重开 |
| 孤儿回合 sweeper | root + revision CAS；只更新投影，不调用模型 | 每个精确 revision 最多一次；已冻结的真实中断交给恢复入口 |
| Objective supervisor、完成门禁恢复 | remediation claim_epoch；signature ledger | 同签名通常 5、transport 8、无进展完成门禁 2；objective 总 claim 20，耗尽登记系统事故 |
| DeliveryRun 恢复 | lease + claim_epoch；同 epoch 不重复计数 | 同签名 distinct claim 预算；既有 parked incident 不再认领 |
| provider / tool recovery | durable attempt、owner permit、side-effect latch | 由 Objective claim 总预算和 provider episode fence 收敛；不因回调刷新预算 |
| 定时 wake | schedule id + next_at CAS；fire_count | 单次计划完成一次；周期计划是显式持续工作，单会话最多 8 条；失败投递预算 3，但成功周期投递无总次数上限 |
| 后台任务结束 wake | task id / wake_state；进程身份校验 | 单任务最多 3 小时、单会话 3 个、全局 8 个；失败投递受工作区恢复预算约束 |
| PR 观测、修复与 §8 收尾 wake | observation reason / closeout_queued；持久 wake id | 并发上限 2、窗口 600 秒，观测频率 ≥180 秒；原因改变可再次投递，不是生命周期 claim 总上限 |

审查未把并发限制、轮询间隔或同进程锁当作跨进程总预算。发现的家族缺口：周期 wake 成功投递没有总 claim 上限；wake enqueue 在推进 CAS 之前完成，崩溃窗口采用至少一次语义；PR 原因反复变化也未有生命周期总 claim 上限。这些行为涉及现有周期提醒/PR 监视语义，本次没有擅改需求。**Spec Amendment（待编排方裁决）**：是否为周期 wake、PR repair 增加独立 durable claim ledger、总次数/截止时间并在耗尽时系统事故停泊；在裁决前 R3 的“每个入口均有硬上限”不能标为完全满足。

## AI Collaboration

- context scope：任务文件、模板、thin 恢复、启动顺序、权威状态派生、既有历史 smoke 和场景注册。
- assumptions：不操作生产库；旧进程 active transport 是中断证据，completed transport 不是。
- review point：先失败回归；启动清理之前冻结证据；同回合 claim 不随更新时间刷新。
- validation result：三周期回归及真实子进程重复恢复回归通过；完整检查以 PR 记录为准。
- 未启动子代理：任务模板明确禁止 `delegate_tasks`，本次采用单执行流。
