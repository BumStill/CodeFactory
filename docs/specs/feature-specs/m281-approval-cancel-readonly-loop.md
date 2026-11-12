# M281：审批被提前取消；只读 `for` 循环被判「无法证明」

- 状态：实现完成（本 PR 交付）
- 关联任务：`task-m281-approval-cancel-and-readonly-loop.md`
- 关联事故：11-09 20:11（M270 会话）、11-11 21:40（M277 会话）

## 1. 现场

### 1.1 一次真实的 `shell.safety` 审批被系统提前作废

编排方在 20:11 看到 M270 会话的一条 `shell.safety` 审批在 1 分钟内消失，工具结果是
`cancelled by user`；随后编排方点了批准，得到的却是
`late or conflicting permission response was rejected`。**没有任何人点过取消。**

### 1.2 只读 `for` 循环仍被要求审批

M277 会话里这条命令被判 `trusted.shell.unproved`，强制走审批：

```bash
for f in a b; do echo "…$(wc -l < "$d/$f")…"; head -60 "$d/$f"; done
```

循环体只有 `echo` / `wc` / `head`，`$(…)` 内也只有 `wc -l < 文件`（只读重定向），列表是字面量。

## 2. 根因（R1）

用 `permission_intents` / `messages` / `chat_run_controls` / `wakeup_schedules` 的落盘记录逐步对齐
（时间均为本地时间，`+08:00`）：

| 时刻 | 记录 | 含义 |
|---|---|---|
| 19:22:01 | `permission_intents` 新建 pending（`shell.safety`） | 审批开始等待 |
| 19:22:01 | 同一行 `expires_at` 为 19:23:01，随后被 `extend_pending` 推到 19:53:01 | 60 秒窗口到期后**正确地**被延长为「保持可批」，不是拒绝 |
| 19:22→20:10 | `messages` 无任何新行，`chat_run_controls.updated_at` 停在 18:41 | 本轮一直停在审批上等待，没有推进 |
| 20:10:14 | `messages` seq117 `status=denied`，`tool_call_id=call_00_…`，正文 `Permission request was interrupted because its response channel closed.` | 同一批次的第一个调用被判「响应通道关闭」 |
| 20:10:14 | `messages` seq118 `status=cancelled`，`tool_call_id=call_01_…`，正文 `Tool call cancelled by user.` | 同一批次余下的调用被判成「用户取消」 |
| 20:10:14 | 该 pending intent 落成 `timed_out` | 审批没有被任何人批准或拒绝就被结算 |
| 20:10:14 | `chat_run_controls` 本轮结算 | 运行被结束 |
| 20:10:15 | 新运行启动，首条 `user` 消息是 `thin-source-repair-…`：`系统自动重试/自愈：接续中断回合`（`计划时间已过（应用重启后补发）`） | 由系统自愈/唤醒接管该会话 |
| 之后 | 编排方点批准 → `late or conflicting permission response was rejected` | 审批对象已经不在 pending 状态 |

结论（三条可复现的机制，都是「非用户触发」）：

1. **系统驱动的回合结束会连带作废在途审批。** 自愈/续跑（`thin-source-repair` 唤醒、
   `running_without_executor` 续跑、Objective 结算的内部 stop）在「本轮正停在用户审批上」时
   仍然把这一轮当成「需要被接续的中断回合」处理：本轮被结束，pending 审批随之被结算成
   `timed_out`，批次里其它调用被写成 `cancelled by user`——这句话没有任何用户动作支撑。
2. **pending 注册表只在等待正常返回时清理。** `pending_permissions` 的条目由
   `request_permission_with_reason` 在 `match response` **之后** `remove`；一旦这次等待所在
   的 future 被丢弃（回合结束、任务被取消、自愈接管），条目留在表里，而它的接收端已经没了。
   同一个 durable pending 审批再被请求时就会撞上这条陈旧条目，被判 `ChannelClosed`
   （seq117 正文），审批于是「还没过期就已经不可批」。
3. 审批 60 秒窗口到期本身**不**是问题：`extend_pending` + 重发 prompt 已经让它保持可批，
   落盘 `expires_at` 证明了这一点。问题在 1、2 两条把「保持可批」的窗口直接作废。

## 3. 规格

### R1 审批在过期前必须保持可批，系统不得替用户取消

- R1.1 pending 注册表必须按「活着的等待」自愈：注册时若同一个 `intent_id` 上留着一条
  **接收端已关闭**的陈条目（上一轮等待的 future 被丢弃留下的），必须用新的等待**接管**该条目，
  而不是把这次调用判成 `ChannelClosed`。仍在等待中的条目（接收端未关闭）保持原有的
  「所有权冲突」拒绝，不允许抢占。
- R1.2 系统续跑/自愈的选择器必须把「本轮正停在一条未过期的 pending 审批上」当成**活着**：
  `running_without_executor` 与 `running_interrupted_by_restart` 必须跳过该会话。
  审批过期（`expires_at` 已过）或已有决定（`allowed`/`denied`/…）时不再豁免。
- R1.3 窗口到期语义不变：到期后仍以 `extend_pending` 延长并重发同一 `intent_id`；
  无人应答记 `timed_out`（不是用户拒绝）。
- R1.4 不得把内部 stop 写成用户取消：`cancelled by user` 只在真有用户停止时出现；
  本条在 R1.1/R1.2 修好「审批被系统作废」后自然成立，本 PR 不新增文案路径。

### R2 字面量 `for` 循环内全部只读即可证明

- R2.1 循环列表是字面量（含 0 个元素的多词列表）、循环体每条简单命令都可证明只读
  （`echo`、`head`、`wc`、`cat`、`ls` 等已在只读表里的命令），且 `$(…)` 内的命令同样只读时，
  整条命令判为可证明（trusted/standard 下都不触发审批）。
- R2.2 反例必须维持原判定：循环体含 `rm`、重定向到文件（`> file`、`>>`）、`git push`、
  `$(…)` 内是写操作、列表不是字面量（`$(git rev-list …)` / 通配符以外的动态来源）时，
  仍然走审批/拒绝。

### R3 不放宽其它判定

- python heredoc 仍需审批；`trusted.heredoc.unproved`、`shell.safety` 的 Ask/Deny 表不变；
  本 PR 只新增「结构内全部只读」这一条正向结论与两条 pending 自愈/豁免规则。

## 4. 测试矩阵

| Req | 测试 | 说明 |
|---|---|---|
| R1.1 | `permission_gateway::tests::m281_retry_after_an_interrupted_attempt_adopts_the_stale_slot` | 丢弃在途等待后同一审批仍可被接管并批准；接收端未关闭的条目仍按所有权冲突拒绝 |
| R1.2 | `thin_restart::tests::m281_pending_approval_session_is_not_a_restart_candidate` | 两个选择器都跳过该会话；已批准/已过期恢复为候选 |
| R1.3 | 同上（虚拟时间 +60s/+120s 断言） | 续期后的审批在原地停留两分钟不被续跑扫描作废；临期延长由 `PermissionIntentStore::extend_pending` 负责（已在等待循环 `permission_gateway.rs:798` 调用，本次未新增真实时钟测试） |
| R2.1 | `permission_gateway::tests::m281_readonly_for_loops_skip_approval`、`agent::trusted_git::tests::m281_readonly_for_loops::read_only_for_loops_are_proved_local` | 现场命令 + `cat`/`ls`/`wc` 变体；trusted/standard 都 Allow |
| R2.2 | `permission_gateway::tests::m281_for_loops_with_writes_still_ask`、`agent::trusted_git::tests::m281_readonly_for_loops::for_loop_with_a_destructive_body_still_asks` | `rm`、`> file`、`>>`、`git push`、非字面量列表、`$(… > file)` 仍要审批/拒绝 |
| R3 | 既有 heredoc / shell.safety 测试 | 全量 `cargo test --lib`、`pnpm test`、`pnpm build` 保持绿 |

## 5. 实现记录

- `permission_gateway.rs`：`create_pending` 在拿到 durable pending 后，若注册表里同一 `intent_id`
  只剩一条**接收端已关闭**的陈条目，就用新的等待接管它（仍在等待的条目维持所有权冲突拒绝）；
  同时重发 pending 意图并顺延 `expires_at`，保证重启/取消后重试仍能弹回同一条审批。
- `thin_restart.rs`：`filter_restart_candidates` 增加 `now_ms`，并在「有没有活执行者」判定之前
  先剔除「本轮停在未过期 pending 审批上」的会话——启动扫描、心跳扫描、`claim_restart` 共用同一入口，
  因此三处口径不会再分叉。
- 只读 `for` 循环的可证明性沿用既有展开 + 逐段判定实现（`trusted_git` / `shell.safety`），
  本次仅补齐现场命令形态的验收覆盖，未放宽任何判定。

## 6. 验收

- 会话内证据：上表测试全绿 + 全量 `cargo test --lib`、`pnpm test`、`pnpm build`。
- 真机验收（编排方在发版安装后完成）：同样的只读 `for` 循环不再触发审批；一条审批停留
  2 分钟以上不被系统取消，过期前点击批准即可继续。
