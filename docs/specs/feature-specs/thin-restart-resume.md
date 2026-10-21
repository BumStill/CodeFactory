# 阶段 2 P0：重启后回合中的 thin 会话被判 ended（objective_already_terminal）

## 现场（v1.91.7，真实会话 `0dcf23e1`）

| 事实 | 值 |
|---|---|
| 会话 | `0dcf23e1-b328-44b4-a21c-0d3170a96ec5`，`delivery_mode=thin` |
| 用户消息 | 04:41:13 UTC（12:41 本地）“用 bash 执行 sleep 60; echo LIVENESS-DONE” |
| 工具调用 | `bash: sleep 60; echo LIVENESS-DONE`，`status=pending`（04:41:15） |
| 应用重启 | 约 04:42:21（回合开始后 **66 秒**） |
| 持久化终态 | `thin_session_state.state=ended`，`reason=objective_already_terminal` |
| 目标 | `9878e4b1`：`legacy_orphan`（04:42:21），回合 `b7367065`：`completed/objective_already_terminal` |

用户看到的：应用重启后，那个还在跑 `sleep 60` 的会话直接显示“已结束（目标已终态）”，
既没有续跑，也没有告诉用户“你被中断了、最后一步是 sleep 60、结果未知”。

## 因果链（已用真实库核对，非推测）

1. 回合进行中，legacy 决策路径（incident controller / 启动收敛）对 thin 会话调用
   `apply_decision`。
2. `retire_legacy_objective_on_thin_session` 把**正在跑的目标**直接退休成
   `legacy_orphan`（该保护本意只针对 `waiting_wake` 的 thin 会话，防止伪造
   “已结束：连续三次…”，但对 `running` 同样生效）。
3. 启动收敛 `reconcile_stale_chat_turns` 的形态 2 认为“终态目标不该拥有活跃回合”，
   把还在跑的回合写成 `terminal_reason='objective_already_terminal'`。
4. `derive_state` 把回合投影的终态当成 thin 的终止事实 → `ended/objective_already_terminal`。
5. 续跑路径（`resume_running_without_executor`，启动时 grace 传 0）被
   `RUNNING_WITHOUT_EXECUTOR_GRACE_MS`（10 分钟）夹住，66 秒的回合根本没被选中。

## 需求

- **R1 终止事实收敛**：thin 会话只允许三种终止事实——最终回答、用户/编排器停止、
  连续三次真实技术失败。legacy objective 的任何终态（含 `objective_already_terminal`
  与重启时的 legacy 结算）不得成为 thin 的终止事实，也不得在重启时写入 thin 会话。
- **R2 启动续跑**：所有“最后状态为 `running` 且无活着执行者”的 thin 会话，在启动后
  自动续跑：追加中断提示（“你在 <时间> 被中断，最后一步是 <工具/命令摘要>（可能已执行
  也可能未执行），先确认现状再继续”），并把 pending 的工具调用标记为“中断，结果未知”。
- **R3 测试先行**：用合成 fixture 先复现“回合中进程被杀 → 重启 → 会话应 running 并续跑”。
- **R4 真机验证**：在真实应用上跑一次“sleep 60 → 重启 → 续跑并给出结果”，附真实证据。

## 决定

1. **不退休运行中的目标**：`retire_legacy_objective_on_thin_session` 在
   “thin 状态为 running 且决策来自非执行者（无 mutation permit）”时**什么都不写**，
   直接返回当前快照。运行中的回合由执行者或 R2 的续跑负责，legacy 决策不得结束它。
   该函数原本的 `waiting_wake` 保护不变。
2. **形态 2 跳过 thin**：`reconcile_stale_chat_turns` 中“终态目标不该拥有活跃回合”
   对 `delivery_mode='thin'` 的会话不适用（thin 的终态由 thin_session_state 拥有）。
   形态 1（活跃目标只保留当前回合）保持不变。
3. **读侧兜底**：`derive_state` 忽略 `objective_already_terminal` 与
   `superseded_at_startup` 两种 legacy 收敛原因，历史已写坏的投影不再能结束会话。
4. **启动扫描不套用心跳宽限**：新增 `running_interrupted_by_restart`（cutoff = 启动时刻，
   仍要求无活执行者、无未消费的停止意图），由 `ResumeOrigin::Startup` 使用；
   心跳继续用 10 分钟宽限（防止把刚开始的回合当死回合）。启动选择不再依赖回合/控制行
   是否存在，避免“控制行被退休后永远选不中”。
5. **中断提示与 pending 工具**：续跑前把该会话 `pending`/`approved` 且无结果的
   `tool_calls` 标为 `error='中断，结果未知：应用重启，工具可能已执行也可能未执行'`，
   摘要明确带上“中断，结果未知”。

## 不做什么

- 不改 thin 的四态、不改 `settle_outcome` 的三次失败规则、不改心跳周期。
- 不给 legacy objective 增加新的终态；不删除兼容记录。
- 不用 UI 可见、HTTP 200 或“进程活着”当完成证据。

## 验收

- `thin_derive`：legacy 收敛原因不再结束 running 的 thin 会话；停止/最终回答/三次失败仍然结束。
- `thin_restart`：启动（66 秒前更新、无活执行者）会被选中，心跳仍等宽限；
  pending 工具调用被标记“中断，结果未知”。
- `objective`：终态目标下的 thin 活跃回合不被启动收敛关闭；运行中 thin 会话的 legacy
  决策不写目标终态。
- 真机（R4）：`sleep 60` → 重启 → 会话保持 running → 续跑输出 `LIVENESS-DONE`。
