# Worktree 生命周期与构建产物控制：使用体验设计

## 日常路径

开发者创建或 checkout 新 worktree 后无需改变 Cargo 命令习惯；hook 只在首次创建共享链接时
输出一行确认。长会话仍推荐使用 `pnpm cargo:shared -- <args>`，以便日志明确显示共享策略。

PR 合并后，从主 checkout 或其他仍在使用的 worktree 执行：

```sh
pnpm worktrees:closeout -- --path /absolute/path/to/finished-worktree --apply
```

成功输出路径和分支；若有未提交内容、没有 merged PR、目标为当前目录或 detached，则失败并
保持目录不变，提示先保存或人工处理。批量巡检继续使用 `pnpm worktrees`。
