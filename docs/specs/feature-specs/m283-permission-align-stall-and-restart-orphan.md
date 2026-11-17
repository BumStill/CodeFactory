# M283 权限请求「暂时无法与当前任务对齐」后的静默停摆，以及重启救援说「可继续」却判会话结束

## 现场（v1.121.0 → v1.122.0，会话 78d0fe52-5060-4d6d-b375-8197b4754180）

| 时间（本地） | 事实（`codefactory.db` 只读取证） |
|---|---|
| 00:35:58 | 模型写出 `## 阻断总结（M277 / R1+R2）`，回合收束 |
| 00:36:03 | 系统按收束结果排了一条 `thin-continue-…` 自动续跑唤醒（00:37:03 触发） |
| 00:36:06 | 用户发「继续，不要停」，开启新回合 `28f54fce`；该条自动续跑唤醒在入场时被取消（正确） |
| 00:36:06.336 | 新回合建出 Objective `5d7b02c9`（`platform_incident`，revision 0），绑定 `chat_root_turn=28f54fce` |
| 00:36:08.194 | 该 Objective 被写成 `legacy_orphan`（revision 1，`completed_at` 为空），此时它名下的回合正在跑 |
| 00:36:11 | 模型第一条 bash 被拒：`权限请求暂时无法与当前任务对齐，尚未执行任何操作；系统自动重试/自愈，工作区和改动保留。` |
| 00:36:12 | 回合被写成 `status='waiting'`、`terminal_reason='permission_channel_closed'`；`permission-reconcile-…` 唤醒被建出（00:37:12 触发） |
| 00:36:12 之后 | 该 `permission-reconcile-…` 唤醒被**静默取消**，且没有任何 `session_audit_events` 记录 |
| 00:36–01:21 | 会话停在 `waiting`，45 分钟无接管、无唤醒、无提示，用户看到「已经结束」 |
| 01:21:46 | 重启救援把 `9f6f0e78` 判成 `legacy_orphan`，15 个回合被批量写成 `settled / completed / orphaned_by_restart`；救援提示却说「会话可以继续」 |

### 根因

1. **R1（身份被提前退休）**：权限准入把「Objective 状态」当成唯一权威。回合 `28f54fce` 仍然是活的（`phase='working'`、`terminal_reason` 为空），但它名下的 Objective 已被退休成 `legacy_orphan`，`resolve_scope()` 的两条查询都匹配不到行，直接 `bail!("permission request requires one authoritative Objective binding")`。真正的权威是「这个正在跑的回合」，不是它的状态字段。
2. **R2（拒绝文案像用户拒绝，且没人接管）**：同一段拒绝既用于「用户拒绝」，导致模型把它读成「被拒/任务终止」并写出阻断总结；同时第一次重试与第二次重试是**同一条 SQL**，中间没有任何核对动作，所以「系统自动重试/自愈」只是文案。`permission-reconcile-…` 唤醒被建出后又被「会话没有未结算的 objective」的清理路径取消，回合停在 `waiting` 且没有任何 owner——这就是 45 分钟静默停摆。
3. **R3（救援只写提示，不恢复状态）**：`restore_registered_branch()` 在 `tx.commit()` 后就返回，只往 `messages` 写一条「会话可以继续」，既不重算 thin 派生状态，也不排续跑唤醒；派生层看到回合全是 `orphaned_by_restart`，于是会话被判 `ended`。提示与真实状态互相矛盾。

## 需求

| ID | 需求 | 判定 |
|---|---|---|
| **R1** | 仍在运行的回合始终有唯一可对齐的 Objective 身份，不因该 Objective 被系统退休而失去权限准入 | 回合 `chat_turn_state.status='working'` 且 `terminal_reason IS NULL` 时，其绑定的 Objective 即使在 `legacy_orphan` 也能解析出 scope |
| **R2** | 对齐失败只影响**这一次**操作，不终止任务；文案明确写系统侧自动重试，禁止模型写阻断总结 | 拒绝内容不含「任务已结束/被拒」语气且含「系统自动重试」；模型被明确要求在同一回合继续 |
| **R3** | 对齐失败进入 `waiting` 时**必须**有 owner：`permission-reconcile-…` 唤醒不得被「无未结算 Objective」的清理路径取消 | 存在 pending 的 `permission-reconcile-*` 唤醒时，清理路径不结束会话、不取消该唤醒 |
| **R4** | 救援提示与真实状态一致：救援成功后重算派生状态，并排一条续跑唤醒 | 救援提交后 thin 派生不再因 `orphaned_by_restart` 判 `ended`，且存在 pending 续跑唤醒 |
| **R5** | 以上路径全部由系统自动重试/自愈，不向用户索取技术性人工动作（产品级中断除外） | 文案与代码路径可直接核对 |
| **R6** | 规格、代码、测试同批入仓；`docs/testing/scenario-registry.json` 声明受影响场景 | PR body 带 `Scenario-Test:` |

## 决策

- **不做**「为退休 Objective 复活新 Objective」的写入型自愈：身份权威是回合，复活会再造一次 revision 竞态。改为在准入层按「回合仍在运行」承认该绑定，零新增写入。
- **不做**绕过权限：R1 只在**同一个**已绑定、仍在运行的回合内承认身份，且仍走完整 intent 落库与复核；用户拒绝语义完全不变。
- 清理路径的豁免只针对 `permission-reconcile-*` 这一族唤醒（系统自己排的权限核对），不放宽「会话已停止即取消全部唤醒」。
- 救援后的续跑复用现有 `thin_state` 机制（`settle_error` + `ModelUnavailable`），不新造调度器。

## 已排除的假设

- 不是用户拒绝：`permission_intents` 中 00:36:11 没有 `denied` 行，只有通道关闭。
- 不是模型擅自停摆：模型在 00:36:11 已开始按用户步骤执行，被工具结果打断。
- 不是重启丢状态：停摆发生在 00:36，重启在 01:21，二者相隔 45 分钟。
