# M107 家族根治：「先对账」机制误伤正常步骤——用 4 个真实现场回放驱动修复

## 为什么是家族根治
同一机制今晚修了三轮（M98 #692 → M103 #705 → M106 #711），每轮只堵住当时看到的一种现象，v1.88.19 上第 4 次复发。本单**不允许**再针对单一现象打补丁；修复必须让下面 4 个真实现场的回放全部按预期继续。

## 4 个真实现场（本机 DB 只读取证，提取为匿名合成 fixture：只保留事件/工具/attempt 的形状与状态，不保留路径、内容、凭据）
1. 3495880a（v1.88.19，02:14–02:16）：12 次 provider attempt 全部 `failed_replayable / provider_response_checkpoint_reconciled`（1 分钟内每一步都走了对账）；完成门禁在步骤之间 3 次 `completion_evidence_incomplete`（blocker 是只读链 `gh run view --job … --log-failed && git show … && git fetch …`）；最后一个工具是 bash（rustc 编译并运行一个**预期失败**的回归测试，输出写到 /tmp），status=error（已结束、有结果）；随后 attempt `response_committed` → 02:16:14 `PROVIDER_RECONCILIATION_UNSAFE` → failed。
2. 4ad3b5f3（v1.88.18，00:59）：最后工具 bash（python 调 gh api GET，只读）status=error → `response_committed` → UNSAFE → failed。
3. 59147b73（v1.88.17，00:25）：最后工具 bash（python 改本任务工作区文件）status=waiting/unknown，用户 steer 后对账 → UNSAFE → failed。
4. 3f0f0236（v1.88.16，22:46–23:01）：external_state_uncertain → attempt `in_flight` 「cannot continue under this live owner」→ 反复 PROVIDER_EPISODE_UNRECONCILED → technical_recovery_exhausted。
- failure_signature：1–3 均为 sha256:4ca854ab…；4 为 sha256:81c23198…。

## 需求
- R1 先写清楚设计：什么情况下需要「对账」（只在进程重启/接管、或确实存在**结果未知的外部写入**时），正常的单进程连续步骤为何会进入对账（现场 1 的 12 次 reconciled），一个**已经结束并有结果**的工具调用（status=done/error）为何会被当成结果未知。写入规格。
- R2 不变量（实现并用测试锁住）：
  a. 已结束（有 result）的工具调用永远不是「未知」，无论成功失败；
  b. 本机文件写入（工作区内或系统临时目录）不是需要防重复的外部副作用；只读命令更不是；
  c. 只有「外部写入（push/PR 写/forge POST/PATCH/DELETE/发版/对外网络写）且无回执或回执未知」才可能导致 UNSAFE 终态；
  d. 正常连续步骤不进入对账路径；
  e. 完成门禁只在 agent 声称完成时评估，不在每个工具步骤之间评估；只读链（含 git fetch、gh run view、git show 组合）不得成为必过 blocker（复用 M52b 的结构化分类，检查它为何没覆盖 `&&` 链和 git fetch）。
- R3 4 个现场各自一条回放测试（先失败）：1、2、3 断言会话继续；4 断言对账后继续；另加一条「真正 push 后回执未知」的反例断言 UNSAFE 终态。
- R4 把回放测试接进 E2E-001 / E2E-007 的自动化（registry 修改本单授权，状态如实）。
- R5 规格里列出此前三轮修复各自的假设与遗漏，避免再犯。

## 约束
- 不放宽外部写入安全边界。合成 fixture，不写生产 DB。

## Implementation Notes（R1 / R5）
- M98 假设 response_committed 只在旧 owner 遗留，解决升级接管但没有区分正常同 owner 连续步骤与异常恢复。
- M103 将串行请求返回视作可对账边界，在每次请求前无条件调用 reconcile_finished_request；因此正常 response_committed 也被改成 failed_replayable。安全判定依赖单调 side_effect_started 和 objective 下未决 receipt，而不是工具终态结果。
- M106 在 provider 边界观察 workspace_git 的有限 Python/forge 形状，漏掉已结束失败命令、临时目录产物和 workspace_file；有限语法无法覆盖所有正常本机操作。
- 设计：正常同 owner、同 revision 的 response_committed 不做恢复改写；异常中断/重启/接管才结算旧请求。先消费已落库工具结果，再检查真正未决的外部写入。已知 error 仍是失败证据，不等于成功，也不因 provider 对账阻止 agent 修正。
- 对未知本机操作只做本机观察，不借本机 diff 证明外部写入成功；外部写且无结果仍禁止重放。
- 完成门禁在无工具调用的最终响应分支评估；工具步骤中只累积证据，不据此拒绝完成。

## Requirements Traceability / 测试矩阵
| Req ID | 要求 | 最低证据 |
| --- | --- | --- |
| R1/R5 | 根因和历史遗漏 | 本规格 Implementation Notes |
| R2a–d/R3 | 四个现场继续、未知 push 停止 | 合成 SQLite 回放及真实 production adapter 测试 |
| R2e | 只读链不阻断、工具步骤不评估终态 | agent-core 分类测试、agent-loop 门禁测试 |
| R4 | E2E-001/E2E-007 接入 | registry 引用实际执行的 Rust 测试，保持 partially_implemented |

## Applicable Harnesses
Spec、Compatibility、Observation、Payload、Release（只合并不发版）、AI Collaboration。无 UI 修改，不启用 Viewport。

## AI Collaboration
context scope：provider transport/store、tool recovery、command classification、agent-loop；assumptions：不改变外部未知写入的禁止重放边界；review point：回执终态与 owner/revision 隔离；validation result：随最终 PR 填写。任务模板禁止 delegate_tasks；本任务保持单执行流，避免多个 writer 修改相同恢复边界。

## 发版后真实应用验收（编排方）
真机验收由编排方发版安装后完成：依次执行预期失败且输出到临时目录的测试、Python GET 失败、本地 Python 编辑后 steer、中断 in_flight 后恢复；核对继续执行、恢复预算不变，正常步骤无 checkpoint_reconciled 事件；未知 push 必须停止且展示工具和交付回执位置。本单不发版，不声称 live。
