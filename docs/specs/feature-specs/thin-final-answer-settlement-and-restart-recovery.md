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

## 已知限制

真机「重启后自动续跑」需要安装包重启才能观察；本地以合成 fixture 覆盖选择与
结算两条判定，真机验证在发布后按验收路径复验。
