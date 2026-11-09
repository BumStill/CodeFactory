# M270：沙箱里用 `cargo:shared` 编译，崩溃了还返回成功

## 背景与用户决策

2026-11-08（会话 833ac8cb，2026-11-09 复核）在信任模式（M224 沙箱）里跑
`node scripts/cargo-shared.mjs`：

- Node 在写共享缓存心跳文件 `/Users/leo/Projects/CodeFactory/.codefactory-cache/cargo-target/.codefactory-in-use`
  时报 `EPERM`，进程带着一串 Node 栈崩溃。
- 后台任务记录的退出码却是 `0`。
- 那条建议（"把 `CODEFACTORY_BUILD_CACHE_ROOT` 指向 workspace"）跑不通：沙箱允许写
  worktree 里的 `.codefactory-cache`，但共享缓存解析到主 checkout
  （`git rev-parse --git-common-dir` 的父目录），主 checkout 不在沙箱可写根里。
  脚本在 `markInUse` 就死了，`CODEFACTORY_BUILD_CACHE_ROOT` 根本不是这个脚本读的变量。
  结果看起来像"已经开启共享缓存"，实际一次编译都没跑。

用户决策：沙箱里应该能正常编译；失败就必须是失败，不能有假成功。

## 已确认的根因（证据）

1. **退出码 0 来自调用方的管道，不来自脚本，也不来自后台任务包装层。**
   事故命令（`background_tasks` 表，`bg-b70b2b75f0c74ec7aa1ff2911afe6caf`、
   `bg-75df0375dd2248b28748adbfbc35c37d`、`bg-594126687d8a4980a892a34b88fc941e`）
   都以 `2>&1 | tail -40` 结尾；`/bin/sh -c` 的退出码取管道最后一段 `tail` 的 0。
   直接运行同一脚本（缓存目录只读）实测退出码为 `1`。
   `src-tauri/src/agent/background_tasks.rs` 用 `status.code()` 原样记录子进程退出码，
   `shell_invocation` 是 `/bin/sh -c "<command>"`，包装层没有吞码。
2. **崩溃点是 `markInUse(sharedDir)`**：共享缓存固定在主 checkout，
   沙箱只允许写当前 worktree，因此心跳写入必然被拒（`EPERM`/`EACCES`）。
3. **降级路径不存在**：脚本没有"共享缓存不可写时改用工作区内的任务专属目录"的分支，
   所以在沙箱里 100% 直接崩溃。

## Requirements Traceability

| Req ID | 要求 | 最低证据 |
|---|---|---|
| M270-R1 | `scripts/cargo-shared.mjs`（含它调用的预算检查）任何失败都以非零退出码结束，不允许假成功。 | 进程级测试：心跳全部不可写时退出码非零；cargo 非零时脚本透传同一码；预算检查失败时构建照跑但最终退出码非零。 |
| M270-R2 | 查清"崩溃却是退出码 0"的根因并留证。 | 上文"已确认的根因"第 1 条：事故命令以 `\| tail` 结尾 + 直跑脚本退出码 1 的实测输出。 |
| M270-R3 | 共享缓存不可写时，自动改用工作区（worktree）内的任务专属目录（`.codefactory-cache/cargo-target-<task>`，同一 `looksLikeManagedCache` 命名与预算入口）并说明原因；两者都不可写时直接报错，错误里给出"在工作区内运行 / 让该目录可写"的办法。不允许静默降级。 | 单元测试：`selectTargetDir` 在共享目录抛错时返回任务专属目录并带原因；两者都抛错时抛出含两种原因与修复办法的错误。进程级测试：共享目录只读时 cargo 收到的是任务专属目录，stderr 说明原因；两者都只读时非零退出且 stderr 含修复办法。 |
| M270-R4 | 修正文档里那条走不通的建议：沙箱内的正确做法是任务专属目录，`CODEFACTORY_BUILD_CACHE_ROOT` 不是共享缓存开关。 | 本规格"实现说明"一节 + AGENTS 的 CF-BUILD 段落指向 `cargo-target-*` 任务目录（已存在），`cargo-shared.mjs` 内注释说明主 checkout 不可写的原因。 |
| M270-R5 | 预算检查失败不得静默：要么构建继续且退出码非零并写明原因，要么不构建并报错。 | 进程级测试：伪造预算检查抛错，cargo 成功（0）时脚本最终退出码非零，stderr 含 `budget`。 |

## Applicable Harnesses

- **Spec Harness**：本文件即规格，Req ID ↔ 测试一一对应。
- **Compatibility Harness**：`selectTargetDir`/`taskTargetFor` 为新增导出，原有导出
  （`resolveCargoCwd`、`stripLeadingSeparator`、`sharedBuildEnvironment`、
  `enforceSharedBudget`、`sharedTargetFor`）签名与语义不变；`--print-target` 行为不变。
- **Release Harness**：`scripts/cargo-shared.mjs` 属于构建工具链；PR 需声明受影响场景。
- **Observation Harness**：降级与失败都在 stderr 留一行可读原因，不静默。
- 不涉及 UI / 视口 / 上传载荷，故不启用 Viewport Harness、Payload Harness。

## 测试矩阵

| 用例 | 覆盖 | 类型 |
|---|---|---|
| 共享目录可写时选共享目录 | R3 正路径 | 单元 |
| 共享目录抛 EPERM → 任务专属目录 + 原因 | R3 降级 | 单元 |
| 两者都抛错 → 抛出含两种原因与修复办法的错误 | R3 硬失败 | 单元 |
| 任务目录命名带 worktree 标识且在 `.codefactory-cache/` 下 | R3 命名约定 | 单元 |
| 共享目录只读 → cargo 的 `CARGO_TARGET_DIR` 是任务专属目录，stderr 说明原因，退出码 0 | R3 端到端 | 进程 |
| 两者都只读 → 未编译任何东西，退出码非零，stderr 含修复办法 | R1/R3 | 进程 |
| cargo 退出 7 → 脚本退出 7 | R1 透传 | 进程 |
| 预算检查抛错 + cargo 成功 → 退出码非零，stderr 含 budget | R5 | 进程 |

## 实现说明（不改变任何需求语义）

- 共享缓存路径仍由 `sharedTargetFor` 从 `git --git-common-dir` 解析（多 worktree 共享一个缓存的原则不变）；
  只在"标记不进去"时降级，不改变正常路径。
- 降级目录放在**当前 worktree** 的 `.codefactory-cache/cargo-target-<tag>`，
  名称符合 CF-BUILD 的 `cargo-target-*` 约定，因此仍受同一预算入口与 LRU 淘汰管辖。
- 降级时预算扫描根是当前 worktree 的 `.codefactory-cache`，不会去动主 checkout 的缓存。
- `CODEFACTORY_BUILD_CACHE_ROOT` 不是 `cargo-shared.mjs` 的输入；文档中不再把它当共享缓存开关。
