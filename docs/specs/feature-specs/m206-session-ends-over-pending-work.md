# M206：会话在有未处理工作时被错误结束（家族审查，一夜三例）

Release-Urgency: immediate

10-30 凌晨 v1.91.61 上一夜之间三单全部「没做就结束」，同属 thin 状态推导家族（M197/M199/M200/M202/M203/M205 之后再次复发，按家族审查处理，不许逐例打补丁）。

## 三例证据
1. **错认 PR**（会话 45191579，M203 实施中）：分支 `codefactory/objective-ab9051…`，从未开 PR。01:28–01:30 工具输出里出现 `M188`、源码行号 `188-` 等文本后，`thin_external_observations` 被写入 PR #188（7 月已关闭、分支 fix/task-blocker-actions，与本会话无关），随后 `thin-repair-…-188` 唤醒注入「PR #188 已关闭但未合并，执行 §8 收尾」，01:35 会话以「PR #188 已关闭…无需继续自动收尾」结束——当时 cargo test 刚跑完待修，工作被丢弃。
2. **续接消息被吞**（会话 5ee1876f）：上一单 PR #917 已合并，会话 ended。01:35:51 通过 dispatch `send`（mode queue）发入新任务（M196），没有任何 assistant 回合，01:36:14 thin 状态再次写成 ended「PR #917 已合并；工作区已清理，无需收尾」。
3. **无重启却判孤儿**（会话 beafe825，M201）：01:40:58 create_and_send，无 assistant 回合；01:42:01 `thin-source-repair` 唤醒「接续中断回合」；01:42:07 ended `orphaned_by_restart`。应用进程自 10-29 22:44 起未重启。

## 要求
1. 三例各造匿名夹具先复现（红）。
2. 家族不变式（全局测试，所有写 ended 的路径都要过）：
   - 最新一条真实用户消息（非系统唤醒）之后没有 assistant 回合时，会话不得进入 ended / orphaned。
   - `orphaned_by_restart` 只能在「该会话最后活动早于本进程启动时间」时写。
   - PR 关联只能来自本会话登记工作区的分支（gh 按 head 分支查）或交付工具记录，禁止从消息/工具输出文本里抽数字。
3. 已结束会话收到新用户消息 = 重新打开并开始新回合（续接派单依赖这一点）；dispatch `send` 到 ended 会话必须返回新回合已开始或明确错误，不得「delivered」后无声。
4. 修复现存：把被错误关联的 PR 观测清掉（只动关联错误的行，保留历史记录）。

## 验收
真机：dispatch 续接一个已结束会话 → 1 分钟内出现 assistant 回合；新建会话 5 分钟内不会出现 orphaned_by_restart；工具输出含 `#188`/`M188` 文本的会话不会被关联到 PR #188。

## Implementation Notes
- R1–R4 对应要求编号；不改变原验收。合成 fixture 覆盖误认 PR、旧合并事实压过新消息、同进程孤儿回收及状态写入家族。
- Harness：Spec、Compatibility、Observation、Release、AI Collaboration；无布局改动，不适用 Viewport。按任务模板单执行流，不调用 delegate_tasks。
- 真机安装验收由编排方另行发版后完成；本任务只交付合并，not live。
