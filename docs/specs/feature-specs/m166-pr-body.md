## 修复与需求映射

M166 R1：保守解析 `;`、`&&`、只读管道、引号、`cd`、字面量变量与 `git -C`；只接受本会话登记且 Git identity 校验通过的精确工作区。允许受限 fetch/merge/stash/pop/status/log/diff 与本会话工作树删除。删除前为所有受影响工作树创建持久备份；执行阶段重复校验与备份。
M166 R2：未知变量、命令替换、重定向、未知 shell 语法、改变 Git/shell 环境的赋值、越界及 force push 保持审批。
M166 R3：合成 fixture 验证两条现场命令形状、多工作树删除后备份内容仍可读取、未登记目标/主仓库/其他工作树与远端改写反例。

## TDD 与自验

- 已先看到失败：`m166_r1_compound_stash_fetch_merge_and_literal_cwd` 在旧实现拒绝组合命令；同轮未知展开/越界反例通过。
- R1：`m166_r1_compound_stash_fetch_merge_and_literal_cwd`、网关 `m159_gateway_session_identity_backup_and_remote_fence` 中新增组合命令 Allow 与 pending_permissions 为空断言。
- R2：`m166_r2_unknown_expansion_and_compound_escapes_require_approval` 与原 M159 越界/远端保护。
- R3：`m166_r3_registered_worktree_remove_snapshots_and_foreign_fences` 实际运行 Git 工作树删除，断言目标消失、备份 ref 内容恢复、未登记/外部路径拒绝。
- 标准模式：`m159_standard_reset_still_requests_approval`。
- 旧测试修正：M166 R1 明确允许 stash pop，因此旧 M159 将 stash pop 一律拒绝的单项断言不再适用；其余保护保留。曾发现 checkout main 保护退化，已限定分支特例为显式 git -C 当前分支。
- 完整验证：pnpm test、pnpm build；独立任务缓存下 cargo test --workspace --no-fail-fast；场景与 README validators。最终结果以执行日志及 CI 为准，不将编译成功视为主路径证据。

## Harness 与协作记录

Spec：原始任务规格与 Implementation Notes 已入库。Compatibility：M159 保护与标准模式回归。Payload：逐字段 shell 解析及拒绝测试。Observation：实际 Git ref、备份内容、审批 pending 状态断言。Release：仅合并交接，不发版。AI Collaboration：context scope 为 trusted_git/permission_gateway/tool_backend 及合成测试；assumptions 为仅静态证明可免审批；review point 为所有权/变量/管道及备份失败关闭；validation result 见测试记录。本任务遵循模板不调用 delegate_tasks，单执行流完成实现与审查。

## 真实应用验收交接

真机验收由编排方发版后完成；本 PR 无布局变更，不新增浏览器截图要求。安装新版本后在信任模式合成工作区运行两条需求命令：核对不弹审批、工具卡 own_task 标记与备份 ref 可恢复；将路径换成主仓库/其他会话或加入 force push 核对仍审批；标准模式复查仍请求确认。后台入口沿用 bash/dispatch，不依赖 GUI 弹窗操作。

## 收尾

合并后核对 merged=true、squash commit 在 origin/main、远端精确分支与本地状态；清理任务专属构建目录，不删除应用管理的工作区。不执行发布。无待决 Spec Amendment。

## M166 续（Windows 反斜杠路径，PR #861 首轮 CI 暴露）

- 现象：同一组合命令在 macOS 放行，在 Windows 仍弹审批。`scenario-execution` 的 `execute-windows` 与 `receipt` 失败，`check-rust` 在 `m159_gateway_session_identity_backup_and_remote_fence` 上断言 `PermissionOutcome::Allow` 实际得到 `Deny(TimedOut)`。
- 根因：Windows 托管工作区路径以反斜杠分隔且必须加引号（`cd "C:\…\ws"`）。分解器（`trusted_git::segments`）把引号内的 `\` 也当转义符拒绝，`segments` 返回 `None`，整条组合命令失去静态证明，退回审批。
- 修正：引号内的 `\` 按字面量处理；引号外仍一律拒绝（转义/续行无法静态确认）；双引号内的 `\"`、`\\`、`\$`、`` \` `` 仍按转义拒绝。R1/R2 的拒绝面（越界、未知变量、命令替换、重定向、远端写）不变。
- TDD：先写 `trusted_git::tests::m166_quoted_backslash_paths_stay_literal_words` 与 `m166_backslash_escapes_outside_quotes_stay_rejected`，在无修复代码上运行到红（`segments=None`），再上修复转绿。
- 可移植性：`m159_trusted_git.rs` 的 shell 执行抽成 `shell()`（Windows 用 Git for Windows 的 `bin/bash.exe`，其它平台 `sh`），仅替换调用方式，断言未改动；这样 R3 的端到端执行在 Windows 也真正跑得到。
- 回归：`m159_gateway_session_identity_backup_and_remote_fence` 1/1、`trusted_git::tests` 2/2、`--test m159_trusted_git` 7/7（合并最新 origin/main 后重跑）。

Scenario-Test: UI-009, HLT-005, SKL-002
README-Update: reviewed
README-Update-Reason: 修复既有信任模式的组合命令作用域判定，不改变公开安装、隐私或平台承诺。
