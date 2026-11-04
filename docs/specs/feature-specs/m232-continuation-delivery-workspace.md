# M232：续接的第二单无法交付：deliver_changes 仍绑定上一单工作目录

## 现场（2026-11-04，v1.91.78，会话 2ee42f66）
- 12:54 用 `@2ee42f66` 把 M227 续接进做完 M229 的会话。M226 的保护生效：PR #968 合并后会话没有被结束。
- 会话把 M227 做在嵌套 worktree `.claude/worktrees/fix-m227-local-commit-route`（分支 fix/m227-local-commit-route），全部测试通过。
- `deliver_changes` 仍绑定上一单的 execution workspace（分支 codefactory/objective-c2a297c4…，其 PR #968 已合并）。会话明确说不能交付新分支，也不肯把改动提交到旧分支，于是停住等人。
- 13:30 由编排方代为提交并开 PR #973。
- 这一单 objective 的 kind=delivery，status 仍是 legacy_orphan；执行工作区只有旧的一条记录。

## 要求
- R1：同一会话开始新一单时，要有明确的工作区归属：要么在上一单 PR 已合并时为新 objective 分配新的 execution workspace 和分支，要么让 deliver_changes 能交付会话当前 objective 登记的工作区。不得把新改动提交到已合并的旧分支。
- R2：先写失败测试：上一单 PR 已合并 + 同会话新任务 + 改动在新工作区 → deliver_changes 交付到新分支，开出新 PR；旧分支和旧 PR 不被改动。
- R3：§8 收尾唤醒只处理上一单，不得清理或覆盖新一单的工作区（M226 的快照要继续有效）。
- R4：工作区绑定不对时，给模型的错误信息要指出可执行的下一步，不能让会话停住等人。

## 验收（真机）
- 用 `@spec:` 续接一单，第二单不靠人工，从实现到 PR 合并全部走完。

Release-Urgency: immediate

## Implementation Notes
- 交付在任何 Git/远端副作用之前，根据本次 root turn/task 的 objective 登记解析并验证 cwd；不使用会话最新工作区猜测，不允许参数传入任意目录，不移动或接管未登记的嵌套 worktree。
- thin 与 durable 共用入口路由；显式 off 和 release_only 保持原语义。
- R1/R2：真实 Git 工作区 + synthetic forge 测试；R3：复跑 M226 快照/收尾隔离测试；R4：验证分支错误提示包含当前目标与重试动作。
- Harness：Spec、Compatibility、Observation、Release（仅合并交付，不发版）、AI Collaboration。
- 真机验收由编排方发版后完成：安装新版本，上一单合并后通过 @spec: 发起第二单，确认工具 cwd/分支、新 PR 和旧 PR 无副作用，并确认上一单收尾不影响第二单。
