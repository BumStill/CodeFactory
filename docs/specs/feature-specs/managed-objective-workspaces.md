# Objective 受管执行工作区规格

- 状态：已批准实施
- 事故来源：本地 CodeFactory 会话在已被合并的旧分支上继续修改，并从用户根 checkout 发起交付，形成重复提交、冲突 PR 与根目录恢复污染。
- 权威关系：本规格细化 `objective-recovery-control-plane.md` 的 CF-ORC-R2、R6、R10、R27、R29、R31；冲突时以后者的 Objective 状态与恢复语义为准。

## 业务目标

用户授权代码修改后，CodeFactory 必须自动把该 Objective 放进唯一、可恢复、可审计的 Git worktree。用户选择的项目目录是仓库入口和只读基线，不是默认写入目标；主 checkout 即使停在旧分支或带有未提交改动，也不得被主 agent、subagent、delivery 或 restart recovery 改写。

## Requirements Traceability

| Req ID | Requirement | Minimum evidence |
| --- | --- | --- |
| CF-MOW-R1 | `Implement`/`Deliver` 主会话在第一次 provider/tool 执行前分配 Objective 唯一 worktree；system prompt、全部工具、测试和 delivery 使用同一路径 | Rust real-git integration + CodeFactoryDev |
| CF-MOW-R2 | worktree 从远端默认分支的最新已观察 SHA 创建；分配失败时零代码写入并进入 system-owned incident，不得回退共享 cwd | dirty-root + fetch failure tests |
| CF-MOW-R3 | `execution_workspaces` 持久化 objective/repo/gitdir/worktree/branch/base/head/state/owner；同一 Objective 重启只允许重附着同一 identity | SQLite restart/identity collision tests |
| CF-MOW-R4 | 分支名按 Objective 唯一生成；已被任意 terminal PR 使用的分支不得承载新 Objective，且不得把旧提交带入新 PR | terminal-branch fixture + fake forge |
| CF-MOW-R5 | subagent 只能在 Objective 主 worktree 下协作或使用其子 worktree，merge-back 目标不得是用户根 checkout | scheduler integration |
| CF-MOW-R6 | `deliver_changes` 必须把 cwd/repo/worktree/branch 与 Objective 的受管工作区逐字段比对；缺失或冲突时在 stage/commit/push 前 fail closed | delivery zero-side-effect tests |
| CF-MOW-R7 | 即使内部提交继续使用 `--no-verify`，交付前也必须原生证明 base 包含当前已观察的远端默认分支 SHA | stale-base tests |
| CF-MOW-R8 | restart recovery 只重附着受管 worktree，不得 `switch/reset/cherry-pick` 用户根 checkout | cross-process fixture + root reflog oracle |
| CF-MOW-R9 | Objective 进入 `completed/cancelled` 时必须在同一事务把 workspace 转为 `cleanup_pending` 并清执行租约；此后不得重新 attach 为活动执行。仅在 worktree clean、canonical PR 已合并时自动 closeout；dirty、未合并、关闭未合并一律保留 | terminal settlement + restart fence + cleanup matrix |
| CF-MOW-R10 | UI 分开显示“本地受管工作区”“远端 PR/CI/合并”“清理状态”，不得用一个红点或 spinner 混合表达 | reducer/component + real App |

## Primary User Path

1. 用户在根 checkout 选择项目并授权实现。
2. CodeFactory 读取 Git identity 与远端默认分支，创建 Objective 唯一 worktree。
3. 聊天运行目录切到受管 worktree，用户根目录的 branch、HEAD、index 和未提交文件保持不变。
4. 实现、测试与 `deliver_changes` 使用同一持久 identity。
5. PR 合并后 Objective 先完成，再由 lifecycle owner 安全清理 worktree 与本地分支。

## Failure Semantics

- 非 Git 项目：保持原 cwd，可本地执行，但明确标记 `workspace_kind=plain_directory`；不得伪造 Git 隔离。
- 历史 Objective 已记录 side effect、但没有 durable workspace identity：只允许只读 grandfather；后续 mutation fail closed 并进入 system-owned recovery，禁止把当前用户 checkout 追认为受管工作区。
- Git identity、fetch、worktree add、reattach 或 delivery binding 失败：`waiting_system/platform_incident`，零代码副作用，不要求用户发送“继续”。
- 现有 worktree 与 DB identity 不一致：只读观察并保留两侧证据，不猜测、不覆盖、不删除。

## Acceptance

- Given 根 checkout 停在旧分支且 dirty，When 新 Objective 获得 Implement 权限，Then 从最新远端默认分支创建独立 worktree，根 checkout 状态与 reflog 不变。
- Given 同一 Objective 的 App 进程重启，When supervisor 续接，Then 重附着原 worktree/branch/gitdir identity。
- Given delivery 从用户根目录或其他 sibling worktree 调用，When Objective 已绑定受管工作区，Then 在 stage 前拒绝且副作用计数为零。
- Given canonical PR squash merged，When Objective 已终态且 worktree clean，Then closeout；dirty 或未合并时保留并显示原因。
