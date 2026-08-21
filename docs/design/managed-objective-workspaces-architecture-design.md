# Objective 受管工作区：架构设计

## 决策

在 chat admission 已得到 opaque `objective_id` 与 `TurnCapability` 后、构造 `AgentLoop` 前调用原生 `ExecutionWorkspaceManager`。对 `Implement/Deliver` 的 Git 项目，manager 从远端默认分支已观察 SHA 创建 worktree，原子持久化 identity，并把 `effective_cwd` 传给整个 agent loop。

```text
session.cwd (project entry)
        |
chat admission -> Objective + capability
        |
ExecutionWorkspaceManager.allocate_or_attach
        |-- observe repo/default remote/base SHA
        |-- reserve execution_workspaces row
        |-- git worktree add unique branch
        `-- verify gitdir/worktree/head identity
        |
AgentLoop(effective_cwd)
        |-- read/write/bash/test
        `-- deliver_changes -> exact workspace binding gate
```

## 持久对象

`execution_workspaces` 是 workspace 生命周期真相源，至少保存：`objective_id`、`session_id`、`repo_identity`、`repo_root`、`git_common_dir`、`worktree_path`、`worktree_identity`、`branch_name`、`base_ref`、`base_sha`、`head_sha`、`state`、`lease_owner`、时间与失败码。`objectives` 和 `delivery_runs` 保留现有投影字段，但不得反向猜测 workspace。

## 分配算法

1. 解析 source cwd 的 top-level、common git dir、remote URL 与默认分支。
2. 有 remote 时执行有界 fetch；选择 `refs/remotes/<remote>/<default>`，无 remote 的本地仓库才允许以当前 HEAD 为 base。
3. 以 repo identity + Objective digest 生成 app-data 路径；分支为 `codefactory/objective-<digest>`。
4. 先写 `allocating` reservation，再执行 `git worktree add -b`；随后重新捕获 identity 并 CAS 为 `active`。
5. 已有记录只接受 exact path/gitdir/branch/repo/base identity；否则进入 incident，绝不另建或回退共享 cwd。
6. 没有 workspace 记录、但 Objective 已有 `side_effect_started=1` 的历史任务不得自动绑定当前 checkout；保持只读 grandfather，下一次 mutation 由系统恢复处理。

## 一致性与恢复

- Git 操作无法与 SQLite 同事务，使用 reservation + observe/reconcile：崩溃在 add 前可重试，崩溃在 add 后按 branch/path/gitdir 重附着。
- 同一 repo 的不同 Objective 可并行，但 `(repo_identity, branch_name)`、`worktree_path`、`worktree_identity` 均唯一。
- delivery preflight 读取 Objective workspace 并比较捕获 identity；任何冲突都发生在 stage 前。
- 本期不自动删除任何 identity 冲突、dirty 或 terminal 状态不完整的目录。

## Trade-offs

- 在 provider 首轮前分配会增加一次 fetch/worktree 延迟，但避免 prompt 指向根目录后产生绝对路径污染。
- 不在首次 mutation 时 lazy 切换，因为同一回合先读根目录、后写 worktree 会产生不可解释的两套文件视图。
- 不依赖仓库脚本或 hook；产品原生实现，脚本只保留开发者手工入口。
