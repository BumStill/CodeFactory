# thin 会话：最终回答结算与重启续跑（阶段 2 P0）

状态：已实现（v1.91.1 现场缺陷修复）

## 现场

v1.91.1 安装重启（10-21 03:16）后两个 thin 会话的权威状态 `thin_session_state`
与用户看到的事实不一致：

- **A（会话 bee5d6b7）**：模型已经给出最终回答（「页面标题是 Example Domain」），
  但 `thin_session_state.state='running'`、`failure_signature='agent loop stopped: Blocked'`、
  `consecutive_failures=1`。用户看到答完的会话，界面一直显示运行中。
- **B（会话 6f5e87a2）**：应用重启时该会话停在 `running`，重启后既没有回合执行者，
  也没有任何恢复动作；阶段 1 R3 的「读现场」恢复只覆盖「用户再发一条消息」，
  阶段 2 R4 的心跳只扫 `waiting_wake`，于是该行永久停在 `running`。

## 根因

1. A：完成闸门在**已经产出最终文本**后以 `StopReason::Blocked` 收束回合，
   而 thin 结算把那句 `stop_reason == Finished` 之外的一切都当成技术失败，
   于是把「答完」写成「失败且仍运行」。故障签名在数据库里可复现。
2. B：thin 会话是 heartbeat-only，`running` 的恢复职责没有任何一处实现承接：
   进程内没有执行者时既没有启动扫描，也没有心跳扫描。

## 决策

- **R1 最终回答只在一处结算**：`thin_state::settle_outcome` 是唯一判定，
  规则是「已产出最终回答（`final_text` 非空）即结算为最终回答」，包含
  `Blocked` 这种有界停止；没有最终回答才记技术失败。调用方不得再自行判断。
- **R5 外部事项不算「等一个人」**：PR/CI/发版/部署/流水线是模型自己能监视的对象
  （`gh pr view`、`background_task_start`、`schedule_wakeup`），
  `thin::is_external_object` 负责分类：
  - 结算时命名外部事项的等待 → `WaitNeed::External{covered:false}`，
    停在 `waiting_wake` 并写明该挂什么监视，**不再**升级成给用户的问题；
  - 只有真正「等某个人/一次确认」的等待仍走 `WaitNeed::Unwatched` → `waiting_user`；
  - 也不再为外部事项生成「需要你决定是否继续等待」的决策行。
  现场依据：会话 6f5e87a2 03:33 进 `waiting_user`，问题是
  「我在等「交付 PR / CI」，但这不是我能自己确认的对象」。
- **R2 `running` 不允许无人看管**：新增 `agent::thin_restart`，
  以权威表为依据挑选 `state='running'` 且**当前进程没有活执行者**
  （`chat_cancels` 内无该会话）的会话：
  - 启动时宽限期 0，逐条续跑；
  - 心跳每 60 秒扫一次，宽限 10 分钟。
  续跑复用持久化根回合（不插入合成用户消息、不新建第二个根回合），
  并追加阶段 1 R3 的中断提示（`thin::resume_prompt`，system 历史），
  让模型先确认现场再继续；续跑回合的结算仍走 R1 的唯一入口。
- 没有任何可续跑回合时，会话转为 `waiting_user` 并明确发问，而不是留在 `running`。

## 验证

- `agent::thin_restart` 合成 fixture：启动/心跳选择规则（含活执行者、宽限期、
  ended/waiting 不入选）、无回合时发问、`Blocked` 有/无最终回答两条结算分支。
- `agent::thin`（含 `thin_state`）既有用例全绿，证明 R1 的调用点改写没有回退
  四状态语义。

## R6：所有终止路径落入 ended(reason)

用户补充现场：bee5d6b7 在 03:45 被 stop，04:18 thin 状态仍为 running。
界面停止与 dispatch stop 必须在持久化停止入口同步结束 thin 状态；取消回合不能算作
一次技术失败。最终回答、连续三次失败、stop、取消都必须留下 ended 与明确 reason。

实现约束：停止意图与 thin ended 在同一事务提交；清除等待字段；迟到错误、最终回答
和普通 save 不得覆盖 cancelled 终态；只有成功接纳的新用户回合可以清理上一轮取消状态。
legacy 不新增 thin 行。合成回归先观察到两条 Running != Ended，再修复并原样重跑。

## R7 与编排方设计修订：从事实推导四态

此前“唯一权威表”和依赖 state=running 的描述由本节替代。唯一判定入口是
`thin_derive::derive_state(db, session)`，优先级固定为：
1. 当前进程 run-control 对应的真实存活执行者 → running；残留 active 行不算。
2. 运行中的 background_task 或未来 pending wakeup → waiting_wake。
3. 未回答的用户问题（权限请求或绑定根回合的问题记录）→ waiting_user；
   后续用户回答或 stop 解除问题，assistant 提问文本不能充当用户回答。
4. 否则 ended；原因取最后的终止事实（回合终止、最终回答或 stop）。

`thin_session_state` 仅作可重算投影。心跳、会话列表、运行状态查询和当前回合水合
共用 derive_state/refresh，旧写入点保留为兼容缓存刷新。恢复动作与状态显示分离，
重启/心跳续跑根据未终止根回合和 run-control 事实选择，不以缓存 running 为前提。

新增回归先观察缓存 Running != Ended 与 assistant 提问被误判结束，再修复并原样复验；
覆盖污染缓存、执行者释放、真实 refresh、未来/过期唤醒、问题回复和停止原因。
R6 最终全量曾通过 1840 测试；推导接入后相关 thin 21 与 session 18 测试已通过，
后续以最后修改后的全量回归为准。未运行安装版主路径，不声明 live。

交付边界：原 PR #789 已合并（2c0ddf573960d2bdb8c72604c1122dfa91ba2f73），
R6/R7 及推导修订作为后续 PR 交付，持续授权上限为 merged，不触发发版。
此前再次询问授权是执行者把“同一 PR”约束误读为“原 PR 合并即授权失效”，不是工具
消费了授权。已核对 resolve_turn_delivery_authorization：会话持久 grant 保持有效，
普通后续消息不会清除，仅明确撤销或只读约束降权；无需改动正确的授权实现。
本任务按模板单执行流，未使用 sub-agent。

## 已知限制

真机「重启后自动续跑」需要安装包重启才能观察；本地以合成 fixture 覆盖选择与
结算两条判定，真机验证在发布后按验收路径复验。
