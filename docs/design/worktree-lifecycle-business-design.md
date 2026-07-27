# Worktree 生命周期与构建产物控制：业务设计

## 问题与目标

并行任务为隔离创建 sibling worktree，但每个 worktree 的裸 `cargo build` / `cargo test`
都会生成独立的 `src-tauri/target`。合并后的 worktree 也没有固定收尾动作，导致磁盘
被已完成任务长期占用，Finder 中又出现多个难以区分的项目根。

本设计把两个动作变成项目的默认交付路径：

1. 新 worktree 自动复用仓库共同的 Cargo target，裸 `cargo` 不再创建独占 target。
2. PR 合并后，执行者从其他 checkout 运行 squash-safe 的单 worktree closeout，删除自己
   已合并且干净的目录与本地分支。

## 成功标准

- 新建 worktree 的 `src-tauri/target` 是指向共同缓存的符号链接。
- 已有真实 target 从不被 hook 自动删除或替换；只给出迁移提示。
- closeout 只接受 GitHub 已合并 PR 作为 squash 合并的证据；不得以 `merge-base` 否定它。
- dirty、当前 worktree、主 checkout、detached worktree 均拒绝自动删除。
- 清理完成后不保留同名 Finder 项目根或本地分支。
