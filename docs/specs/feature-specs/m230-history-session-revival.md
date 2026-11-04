# M230（紧急）：v1.91.76 把 86 个早已结束的会话复活成「继续当前任务」

## 现场（2026-11-04 09:12，装 v1.91.76 重启后）
- `thin_session_state` 中 86 个历史会话（10-19 至 11-04，PR 全部已合并）由 ended 变成 waiting_wake，理由「PR #N 已合并；继续当前任务」。每个都带一条 `thin-closeout-*` 唤醒，正文要求「请继续当前任务，不要收尾、不要结束会话」。全部是信任模式。
- 当时没有可用模型，所以没有实际执行。编排方用 dispatch `stop` 停了 78 个（名单：agent-tools/m226-revived-stopped-*.txt）。另有 6 个返回 `unknown session`，停不掉，仍是 waiting_wake：1a6f4ab6、f5e88050、bb13ce6d、55b0c2e3、1be7f704、6ce8ef26。
- 根因（M226，commit 6bd252d9b，thin_derive.rs）：`delivery_closeout_may_end` 只把 `completed|cancelled|failed` 算作终态。而有合并记录的会话里，最新 objective 为 `legacy_orphan` 的有 77 个，`cancelled` 只有 5 个，今天的 98724ebc 也是 `legacy_orphan`。于是 may_end=false，continue_current=true。该函数下方的注释已写明 objective status 不可靠，但代码仍以它为依据。M226 的测试没有覆盖 legacy_orphan。
- 影响还在继续：今后每个正常合并的会话，只要 objective 落在 legacy_orphan，都可能被判成「继续当前任务」。

## 要求
- R1：`legacy_orphan` 不得算作「还有当前任务」。先用测试复现：最新 objective 为 legacy_orphan、PR 已合并、工作区已清理、无回合、无排队消息 → 必须走收尾或结束，不得 continue_current。这个测试要先失败。
- R2：M226 想保护的真实场景不能丢：合并后确实有更新的、未到终态的非 legacy objective，或有运行中回合、排队消息、未提交工作时，会话继续，不被结束。
- R3：已经 ended 的会话不得因为重新推导历史 PR 事实而变回 waiting_wake。只有新观察到的事实或用户、系统的新输入才能改变状态。测试：一批历史 ended 会话 + 重启推导 → 0 个状态变化、0 条新唤醒。
- R4：启动时自愈：把仍带这条错误唤醒的会话结算回 ended，取消其 pending 的「继续当前任务」唤醒。这必须覆盖 dispatch 返回 unknown session 的那 6 个，并查清它们为什么在 `sessions` 表里有行、dispatch 却不认。不得删除消息、objective 或工作区记录。
- R5：结束文案写真实结果，不写「用户已停止会话」这类与事实不符的话（78 个被编排方 stop 的会话除外，它们确实被停止了，无需改写）。

## 验收（真机）
- 装新版并重启后：`select state,count(*) from thin_session_state` 中 waiting_wake 只剩确有任务的会话；`wakeup_schedules` 没有 pending 的「继续当前任务」历史唤醒；模型恢复后这些历史会话没有新回合、没有新 model_route_attempts。
- 再做 M226 原验收：`@spec:` 续接一单，上一单合并后后一单继续，不被结束。

Release-Urgency: immediate

## Implementation Notes
- Harness：Spec、Compatibility、Observation、Release（仅合并交接）、AI Collaboration。无 UI 布局改动。
- 单执行流：任务模板禁止 delegate_tasks，避免并行修改状态收敛路径。
- 真机验收由编排方发版后完成；本单不操作用户数据库、不发版。
- R1/R2：真实 SQLite 夹具验证 legacy、非 legacy 活目标、排队消息和嵌套 worktree；保留 M226 既有回归。
- R3：批量 ended 投影重推导，逐字段及唤醒行数断言；新用户输入反例。
- R4/R5：启动自愈覆盖未载入前端列表的持久会话；幂等、无删表记录、真实结束文案和不创建模型回合。
