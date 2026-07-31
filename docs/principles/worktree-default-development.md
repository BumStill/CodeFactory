# Worktree 默认开发规范（Worktree-Default Development）

> 状态：生效 · 适用范围：本仓库任何代码/配置/文档改动 · 上游治理：`codex-delivery-governance`
> 原则出处：`docs/principles/release-cadence.md`（合并 ≠ 发版）与本规范（主 checkout 干净 = 可随时发版）

## 为什么

历史上「直接在主 checkout 上开发」反复造成三类事故：

1. **交付链被卡**：在 `main` 上提交后，交付工具拒绝「从默认分支向自身开 PR」，被迫切分支 + 复位 `main`，绕一大圈；
2. **工作区污染**：主 checkout 里堆着未提交 WIP（stash / 半提交 / 冲突中间态），与 merge、交付搅在一起，产生大量无关冲突，还可能把无关改动混进交付；
3. **手动擦屁股**：改完还要手动清理分支、同步 main、核对残留 worktree。

Worktree 默认开发让这三类问题**结构性消失**：主 checkout 永远干净可发布；开发永远在独立分支上；合并后自动回收，无需人工记账。

## 核心规则

### 1. 分级执行

| 任务类型 | 执行环境 |
|---|---|
| 非平凡任务（多文件改动、任何会进 PR 的代码/配置/文档改动、发布/交付链、并行开发） | **强制 worktree** |
| 只读分析、极小单文件改动、用户明确要求单执行流 | 允许主 checkout 直改 |

判断标准一句话：**只要这个改动会进 PR，就必须从 worktree 开始。**

### 2. 标准生命周期

```
任务开始  → pnpm worktree:start <branch-name>   # fetch main → 开 worktree（自动共享 cargo target）
开发验证  → 全部在 worktree 内完成，主 checkout 不碰
交付      → worktree 内 deliver_changes → PR → CI → squash 合并
合并后    → pnpm worktrees:closeout -- --path <worktree 路径> --apply   # 自动删 worktree + 本地分支
```

### 3. 主 checkout 只做两件事

- **验收**：跑测试、看结果；
- **发版**：触发 auto-release / release 流水线。

任何中间态（WIP、stash、半提交、未合并分支）都不允许长期停留在主 checkout。

### 4. 例外与安全阀

- 只读 / 极小改动 / 用户指定单执行流时跳过 worktree，**但最终输出必须写明原因**；
- 紧急热修允许 `CODEFACTORY_SKIP_SYNC_GATE=1 git commit ...`，但必须标记 `hotfix bypass` 并补 PR + CI（见 AGENTS.md 同步门禁）；
- closeout 只认 GitHub 已合并 PR（squash 语义），拒绝删除 dirty / detached / 未合并的 worktree，绝不误删主 checkout。

## 命令速查

```bash
# 开始：从最新 origin/main 开 worktree（分支名如 fix/foo、feat/bar）
pnpm worktree:start fix/my-change

# 结束：PR 合并后自动清理（脚本会校验「该分支确有已合并 PR」）
pnpm worktrees:closeout -- --path .claude/worktrees/fix-my-change --apply

# 批量清扫超过 7 天的 stale worktree
pnpm worktrees:clean

# 共享 cargo target（worktree 内编译一律走共享缓存）
pnpm cargo:shared -- <cargo arguments>
```

## 验收标准

- 提交前 `git fetch --prune origin main`，当前分支包含最新 `origin/main`（pre-commit hook 强制）；
- 交付走 PR → CI → squash 合并 →（feat/fix 时）auto-release 切版；
- 合并后 closeout 清理无残留：`git worktree list` 无本分支、`git branch` 无本地分支、主 checkout 与 `origin/main` 同步。
