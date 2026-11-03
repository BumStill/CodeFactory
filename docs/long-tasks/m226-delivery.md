# M226：续接任务与嵌套工作区救援

Scenario-Test: HLT-001, HLT-004, RTE-004
README-Update: reviewed
README-Update-Reason: 修复既有任务生命周期及清理安全，不改变公开功能承诺。

## 需求与验证
- R1：当前目标未终态、运行回合、待处理消息均阻止交付事实结束会话。测试 m226_active_current_objective_blocks_terminal_delivery_even_before_merge（已先观察失败）、m226_previous_pr_merge_never_ends_session_that_started_new_objective。
- R2：清理前永久 rescue ref 保存未跟踪文件和 HEAD 祖先；Rust cleanup/build-cache、CLI closeout/GC/build-cache 接入失败即保留。测试 m226_cleanup_snapshot_preserves_nested_worktree_work；scripts/worktree-rescue.test.mjs（入口覆盖先红）。
- R3：上一单合并后仍继续当前任务，不显示“无需收尾”。由 thin 派生回归检查。
- R4：合成时间线、真实临时 Git 仓库验证嵌套未提交文件可由 refs/codefactory/rescue 恢复。

## Harness / AI Collaboration
- Spec：版本化原始需求；Compatibility：thin/cleanup 回归；Payload：真实 Git 未跟踪文件与提交祖先；Observation：真实 ref 内容断言；Release：仅合并交接，不发版。
- Context scope：thin 派生与所有清理入口。Assumptions：旧 PR 不代表当前目标完成。Review point：快照失败不得删除。Validation result：验证执行中。
- 单执行流：任务模板禁止 delegate_tasks，未启用并行角色。

## 自验收与真实安装版分工
- 本单无界面布局变更，无浏览器截图需求。
- 真机验收由编排方发版后完成：安装发布版，以 @spec: 续接 B 任务；合并 A；确认 B 不 ended；在嵌套 worktree 写未提交及未跟踪文件；触发清理；检查永久 rescue ref 的文件内容。
- 无 Spec Amendment；未擅自降低 R1–R4。

Release-Urgency: immediate
