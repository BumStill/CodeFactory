# M106：核对「上一步操作」时按实际副作用分类——本地编辑与只读查询不得被判为「无法确认、停止」

## 现象（真机，v1.88.17/1.88.18，M103 #705 合并后出现）
- 4ad3b5f3（E2E-004 接手，PR #710）00:59：最后一条工具调用是 bash `python3 - <<'PY' … subprocess.check_output(['gh','api','repos/BumStill/CodeFactory/pulls/71…'])`（只读查询 PR），状态 error；随后 provider attempt `response_committed`，对账结果 `PROVIDER_RECONCILIATION_UNSAFE: 无法确认上一步外部操作是否完成，已停止以避免重复执行` → failed。
- 59147b73（PR #704）00:25：上一步是 bash `cd src-tauri/src/agent && python3 - <<'PY' … Path("delivery_scenario_repair.rs").read_text() …`（改本任务工作区内的文件），状态 waiting/unknown；同样被判 PROVIDER_RECONCILIATION_UNSAFE → failed。
- 两次 failure_signature 相同（sha256:4ca854ab…）。正常开发中的一次工具失误就让整单终止。

## 需求
- R1 用数据确认：对账时依据什么判断「外部操作」，为何这两条命令被归为不可确认（读 #705 的实现）。
- R2 按实际副作用分类，而不是「bash = 外部操作」：
  - 只读（查询 forge/git/文件，无写方法）→ 可安全重放，直接继续；
  - 只改本任务受管工作区内文件 → 用工作区 git 状态核对（diff/hash），结果可确定，按已发生/未发生继续，不判 unsafe；
  - 真正的外部写入（push、PR 写操作、forge POST/PATCH/DELETE、发版、对外网络写）→ 走交付回执对账；只有回执确实无法判定时才进入 R4 终态。
  - 复用 M52b 的结构化命令分类（gh 子命令 + 方法 + 写参数），不要再写一份关键词表。
- R3 判定为可继续时：不计恢复预算，把上一步结果（或「需重新执行」）作为下一轮输入交给 agent。
- R4 保留 M103 的安全边界：外部写入且回执不明 → 明确终态，文案说清是哪一个外部操作、成果在哪。
- R5 测试（先失败）：用这两条真实命令形状（含 heredoc python 调 gh api GET、python 改工作区文件）+ 一条真正 push 的反例；断言前两者继续、后者在回执不明时终止。
- R6 能挂进 E2E-007 的自动化就挂（registry 修改本单授权，状态如实）。

## 约束
- 不放宽外部写入安全边界。合成 fixture，不写生产 DB。

## Implementation Notes
- R1：`reconcile_finished_request` 仅检查 attempt 锁存与 objective 下所有 started/unknown receipt，未先运行 `ToolRecoveryStore`。`classify_command` 将 executable heredoc 保守归为 Mutation；通用 bash observer 已具备 workspace_git diff/hash，但 provider 边界未消费其观察结果。
- 不把「工具意图出现在 provider stream」当实际执行证明；回执与受管工作区快照才是本地执行证据。外部写入不得凭本地快照结算。
- Harness：Spec、Compatibility、Observation、Payload、Release（仅合并，不发版）、AI Collaboration。无 UI 变更，不追加 Viewport。
- 真实安装版验收由编排方发版后完成：复现 Python GET error、本地 Python edit unknown，核对自动继续且 recovery attempt_index 不变；未知 push 须明确停止、保留工具/交付回执与成果位置。
