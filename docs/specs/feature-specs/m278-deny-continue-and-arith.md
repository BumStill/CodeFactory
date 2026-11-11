# M278：被拒后的第一句提问要直接续跑；`$((…))` 纯算术应判可证明

## 背景与用户决定

现场：v1.119.0，测试会话 `5b60f8cd`（11-11 05:3x）。

- M276 已经让审批被拒的工具结果带上「只拒这一条，任务没取消，换办法继续」，会话也不会再以
  「阻塞总结」结束（系统 60 秒后自动唤醒继续）。但模型被拒后的**第一条回复**仍然写
  「需要你做的一件事」向用户提问，然后停下等待。这一句对用户是噪音，还白白浪费一个回合和 60 秒。
- 模型按说明改用 `echo $((1+1))`，被判 `trusted.shell.unproved` 再次要审批。纯 shell 算术展开
  没有任何副作用，不该进审批。

用户决定：

- 根因优先，**不许靠在提示词里加关键词表**（不许再往 `BLOCKED_SUMMARY_MARKERS` 这类清单里补词），
  要在**回合结算逻辑**上判断。
- 只放宽 `$((…))` 纯算术这一项；python heredoc 仍需人工审批（用户 11-07 决定保留）。

## 决定（Decisions）

1. **R1 用结算事实而不是措辞**：本回合存在真实的「审批被拒」权限事实（`permission_intents` 的
   denied 行），且任务尚未交付，且没有停止意图 —— 这时回合以提问 / 阻塞总结 / 普通最终回答
   任何一种形式结束，都由系统**立即**挂续接唤醒继续（不延迟 60 秒），并且不把会话投影成
   `waiting_user`。判断依据是权限事实与回合边界，不是助手文本里出现了哪些词。
   - 实现修正（原稿写的是「被拒且用户附了说明」）：M276 的用户说明只随工具结果落进消息内容，
     `permission_intents` 里没有说明字段。要按「有说明」判定就只能读文本，正是本规格禁止的做法，
     因此判定条件收敛为「存在 denied 行」；有没有写说明一视同仁。
   - 只接续一次：唤醒 id 绑定该 denied 行（`thin-continue-{session}-{denied_at}`），已兑现的唤醒
     不再重排，避免把会话变成无限自动重试。
2. **R2 用白名单文法判定**：`$((…))` 内只允许字面整数、已赋值变量（字母/数字/下划线标识符）、
   运算符 `+ - * / %`、括号与空白。出现命令替换 `$(…)`、反引号、`; | & < >`、引号、其他
   命令名或任何 shell 元字符 → 仍按原判定（`trusted.shell.unproved`）。
3. **R3 不放宽任何其他权限判定**：python heredoc、`$(…)`、反引号、重定向、赋值以外的副作用
   全部维持原行为。

## Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| M278-R1 | 存在真实 denied 事实、任务未交付时，模型以提问/阻塞总结结束回合 → 系统立即续跑，不等待 60 秒，且会话不投影成「等待用户」 | 真实回合结算测试：合成 denied 行 + 助手提问，断言状态为 `waiting_wake`、`wake_source` 指向**到期时间 ≤ 现在**的 pending 唤醒、唤醒词不含助手原话；反例（没有 denied 行）不挂续接；重复结算不重排同一行 |
| M278-R2 | 只含字面数字、已赋值变量与 `+ - * / %`、括号的 `$((…))` 判为可证明（无副作用）；含命令替换、反引号、其他副作用仍走原判定 | 回归表测试（正例 `echo $((1+1))`、`x=$((a*2))` 免审批；反例 `echo $(($(rm -rf x)))`、`` echo $((`id`)) `` 仍需审批），并覆盖 `$((x+1))` 之类的变量算术与非算术字符串 |
| M278-R3 | 不放宽其他权限判定（python heredoc 仍需审批） | 已有的 heredoc 审批测试保持绿色；新增回归断言 heredoc 命令仍判 `trusted.shell.unproved` |

## Applicable Harnesses

- **Spec Harness**：本文件即规格；Requirements Traceability 与测试矩阵逐条对应。
- **Compatibility Harness**：不改变任何已有权限判定的默认拒绝方向（只把纯算术从「需审批」
  提升为「可证明」），`cargo test --workspace` 全量回归覆盖。
- **AI Collaboration Harness**：context scope = 任务文件 + `thin_derive.rs` 结算路径 +
  `permission_gateway.rs` 判定路径；assumptions 见下；review point = 「结算处不用措辞」；
  validation result 见 PR。

## 假设与范围

- 拒绝事实的权威来源是 `permission_intents`（真实网关写入），助手文本只是观察结果，不参与判定。
- 「任务未交付」沿用已有结算事实（`unfinished_delivery` / 已完成的交付回执），不新增文本清单。
- 不改 `docs/testing/scenario-registry.json`；若场景门禁缺映射，只追加
  `docs/testing/scenario-mappings.json`。

## 测试矩阵

| 用例 | 类型 | 期望 |
| --- | --- | --- |
| 被拒（带说明）+ 回合以提问结束 + 未交付 | 集成（真实网关 + 结算） | `waiting_wake`，续接唤醒已到期（无 60 秒延迟），非 `ended`/`waiting_user` |
| 被拒（带说明）+ 回合以阻塞总结结束 + 未交付 | 集成 | 同上（与第一条共用同一结算分支） |
| 没有 denied 行、只有助手提问 | 集成 | 不挂续接唤醒（反证：不被措辞触发） |
| 同一条 denied 行重复结算 | 集成 | 已兑现的续接不再重排（只接续一次） |
| 被拒但已交付完成 | 集成 | 正常 `ended` |
| 用户已停止会话 | 集成 | `ended`（cancelled），续跑不覆盖停止意图 |
| `echo $((1+1))`、`x=$((a*2))` | 单元回归 | 可证明（免审批） |
| `echo $(($(rm -rf x)))`、`` echo $((`id`)) ``、`echo $((x=1))`、`echo $((arr[$(id)]))` | 单元回归 | 仍需审批 |
| python heredoc | 既有 + 回归 | 仍需审批 |

## 验证结果

- `cargo test --lib m278`：4 passed（R1 三条结算用例 + R2 回归表）。
- `cargo test --lib agent::thin_derive`：118 passed。
- `cargo test --lib agent::trusted_git`：见 PR 记录。
- 注意：`m260_unparseable_structure_fails_closed` 里原先的 `for c in a b; do echo $((1+2)); done`
  靠「算术未证明」才失败关闭，循环本身是有模型的（同一个测试模块的 `LOCAL_STRUCTURES` 就按内容
  判定循环），纯算术证明后这条不再属于「未建模结构」，已移除并写明原因；其余三条未建模结构
  仍要求失败关闭。
