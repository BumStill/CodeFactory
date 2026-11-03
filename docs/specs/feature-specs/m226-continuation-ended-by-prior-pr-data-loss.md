# M226：续接任务被上一单 PR 合并判成 ended，工作区清理把未提交的新工作一起删了（数据丢失）

## 现场（11-04 04:03，v1.91.74）
- 会话 22e55d80 先做 M191（PR #923）。编排方在 03:37 用 gate-dance 合并了 #923，之后同一会话按 `@spec:` 续接语法开始做 M192，并且已经写了规格章节、11 条失败测试和一部分实现，全都没提交。
- 04:03 thin_session_state 变成 `ended`，reason 是「PR #923 已合并；工作区已清理，无需收尾」。会话最后一条消息是「Now 写失败测试（T1–T11）」，正在干活。
- 工作区 execution-workspaces/94eef8f7b2987cbf/b7a39072… 被删除；会话在里面建的嵌套 worktree `.claude/worktrees/fix-m192-kb-search-cjk` 也一起没了。本地分支 `fix/m192-kb-search-cjk` 指向 origin/main，没有任何提交。refs/codefactory/rescue/ 下没有新的快照：M161 的救援只认登记过的工作区，嵌套 worktree 和未提交改动都没有被保存。
- 这和 M211 是同一类问题（stash 标题「续接会话不再被上一单 PR 合并判成 ended（M211）」），M211 没有覆盖到 gate-dance 合并或续接这条路径。

## 要求
- R1（根因）：判断会话「已结束」，必须看会话**当前的任务/目标**。上一单的 delivery ref（thin_delivery_refs）已合并，只能结算那一单，不能结束一个已经开始新目标的会话。结束必须同时满足三条：当前 objective 已到终态、没有正在运行的回合、没有排队的消息。
- R2（数据安全，最高优先级）：清理任何工作区之前，要先对它和它内部所有的 git worktree（`git worktree list`，包括嵌套在工作区目录里的）做救援快照（M161 的 rescue_snapshot），含未跟踪文件，写成 refs/codefactory/rescue/…，然后才能删除。只要其中有未提交改动或未推送的提交，就必须先完成快照；快照失败就不删。这条也适用于所有其他清理入口：closeout、GC、build-cache。
- R3：用户可见的结束文案不能写「无需收尾」这种和事实不符的话。如果是因为上一单合并而结算，就写「PR #923 已合并；继续当前任务 M192」。
- R4：测试用本次的时间线：A 单 PR 已合并 → 同会话续接 B 单 → B 单在嵌套 worktree 里有未提交改动 → 跑一次 thin 派生和清理。断言：会话不是 ended；如果真的触发了清理，rescue ref 里能还原出那些改动。

## 验收
- 真机：用 `@spec:` 续接一单。上一单合并后，后一单继续进行，不被结束。人为触发一次清理后，`git for-each-ref refs/codefactory/rescue` 能看到快照，内容含未提交文件。

Release-Urgency: immediate

## Implementation Notes
- 真实安装版验收由编排方在发版安装后完成；本单不发版。
- Harness：Spec、Compatibility、Payload、Observation、Release（合并交接）、AI Collaboration。
- 不使用并行 agent：任务模板要求单执行流、不调用 delegate_tasks。
- 测试矩阵：R1/R3 → thin 派生与终态守卫；R2 → 真实临时 Git 仓库、嵌套 worktree、未跟踪文件、未推送提交、快照失败保留；R4 → 同一时间线集成验证。
