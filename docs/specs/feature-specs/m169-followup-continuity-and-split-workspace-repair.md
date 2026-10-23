# M169 追问连续性与会话分裂工作区修复

## 问题

两个真实缺陷，都发生在「用户已经有一个进行中的 Objective」之后。

1. **追问另起炉灶**：Objective 处于等待态（`waiting_system` / `waiting_core_input` /
   `waiting_authorization` / `waiting_business_decision`）时，用户再发一句话（追问、
   补充信息、或对等待项的答复），新的 turn 没有绑定回同一个 Objective，系统当成一次
   全新任务处理，于是同一个用户目标被拆成互不相干的两段，等待中的交付也再没人推进。
2. **分裂后丢失工作区归属**：会话被修复/分裂时，工作区（worktree / 分支）没有跟随
   真正持有该 Pull Request 的那一段，导致后续交付在错误的目录里找 PR。

## 决策

- **连续性由状态判定，不由措辞判定**：是否续接只看会话内 Objective 身份与状态，
  完全不依赖 message 关键词、也不解析 id 字符串。这样「继续」「然后呢」「可以」
  这类说法都能正确续接，而真正的业务决策仍走既有授权路径。
- **作用域严格按 session**：Objective 归属通过 `objectives.session_id` 与会话内
  turn 双重限定，跨会话绝不串用（同一进程里两个会话各有等待中 Objective 时互不影响）。
- **身份不唯一时不拒答**：同一会话存在多个可续接候选时，按「系统侧簿记问题」处理，
  正常回答用户消息，绝不因为内部记账歧义而拒绝用户的下一句话
  （OBJECTIVE_MANUAL_TECHNICAL_HANDOFF：技术态自愈，人工只处理产品级决策）。
- **交付状态只做驱动细化**：DeliveryRun 状态用于在 Objective 已被唯一选中之后细化
  诊断驱动（`recoverable_waiting_open` / `completion_arbitration_open` 等），
  不能反过来参与「选中哪个 Objective」。
- **工作区跟随 PR 归属**：会话分裂/修复时，持有该 PR 的分段保留工作区与分支；
  遗留孤儿若有打开的交付，则把工作区交接给承接方，而不是各自拿到一份。

## 实现要点

- `commands/chat.rs`：入场判定在既有事务内解析连续性（`AlreadyBound` / `Unique` /
  `Ambiguous` / `None`），续接时写入 continuation 分段与 turn 绑定；
  `open_delivery_chat_anchor` 按 session 选出可挂回的交付锚点。
- `agent/objective.rs`：`open_delivery_objective_id_in_tx` 提供事务内的开放交付
  Objective 查询；`ObjectiveStore` 在分裂/修复路径上补齐工作区与 objective 归属。
- `agent/execution_workspace.rs`：`repair_session_split_locked` 与
  `inherit_predecessor_locked` 在分裂修复时保留持有 PR 的工作区，孤儿交付工作区
  向承接方交接。

## 验收

- 等待态 Objective 下，追问消息绑定到同一 Objective，不产生第二个 Objective。
- 两个会话各自有等待中 Objective 时互不干扰。
- 同一会话存在多个候选时不拒答、不误绑。
- 会话分裂修复后，持有 PR 的分段仍然指向原工作区；遗留孤儿的工作区交接给承接方。
- `cargo test --lib`：1981 passed / 0 failed。
