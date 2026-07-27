# Worktree 生命周期与构建产物控制：架构设计

## 共同 Cargo target

`scripts/worktree-cargo-target.mjs` 从 Git common-dir 推导仓库根，再把共同 target 定位到
`<repo>/.codefactory-cache/cargo-target`。`post-checkout` hook 调用该脚本：当
`src-tauri/target` 缺失时创建目录符号链接；已存在目录或指向其他位置的链接一律保持不变。
这使新 worktree 的裸 Cargo 调用与 `pnpm cargo:shared` 落到同一个缓存。

Hook 通过版本化 `.githooks/` 发布，仓库 common config 使用
`git config core.hooksPath .githooks` 激活。hook 失败不能中断 checkout，但会输出可行动提示。

## Squash-safe closeout

`scripts/worktree-closeout.mjs --path <worktree> --apply` 读取 Git worktree 注册表并查询
`gh pr list --state merged --head <branch>`。仅在存在已合并 PR、目标干净、目标不是当前或
主 checkout 时，才执行 `git worktree remove --force`、`git branch -D` 和 `git worktree prune`。
该路径不使用 `merge-base` 判定 PR 合并状态。
