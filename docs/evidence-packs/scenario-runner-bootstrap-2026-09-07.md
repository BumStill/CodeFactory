# 场景执行目标与平台路由的最小治理更新

## 授权与范围

用户于 2026-09-07 明确批准本次 external governance bootstrap。对应 CF-STG-R23、CF-STG-R26；本次仅修复既有执行目标及校验，不代表完整 Bootstrap-1 的 case receipt、implementation digest 和 E2E-004 接入已经完成。

基线为 `10d834516bfd8a2a9d92a53be94bc09ee02893bf`。PR #504 的 head `e837cd1895c7a69d346b9c7f39dfdead289ed049` 在 Actions run `33662213139` 中，Windows 回执记录 18 项失败：11 个 acceptance 源文件被当成 Vitest 用例，7 个 `#[cfg(unix)]` Rust 用例在 Windows 上没有运行。正式 unattended smoke 和 Windows Rust CI 已通过。

## 修改设计

- 从 `automated_by` 删除 11 个不可独立执行的 acceptance 源文件；各场景已有的 `pnpm:test:*:headless` 或 composer workflow 入口保留，实际源码文件不删除。
- 7 个 Unix-only Rust 用例通过既有 `target_runners` 显式派发到 `macos-14`；Windows 专用与跨平台目标保持原分配。
- `path:` 校验按现有 `vite.config.ts` 的 `src/**/*.{test,spec}.{ts,tsx}` 范围识别，拒绝普通源码、备份文件、错误扩展名和越界路径。
- 保留逐目标成功回执要求；不把 Cargo 零用例运行判为通过；不提升任何 Complex E2E 的状态或证据等级。

## 验证与执行顺序

1. 原两项回归先红后绿；追加扩展名/目录反例后，旧条件稳定产生五项失败，新条件通过。
2. 完成独立只读审查和场景治理、catalog、receipt、ruleset 回归；创建独立治理 PR。
3. 在 ruleset 保持完整的情况下等待其余 required checks。旧 trusted gate 应仅因 registry 与 validator 两个 trust-root 文件变化拒绝该 PR。
4. 外部执行者复核完整 diff、PR head、main SHA 和五项 required checks，保存线上 ruleset 原始快照。仅移除 `scenario-gate-pr` required context，保留 active、strict、无 bypass actor、PR 与防删除/强推规则。
5. 合并治理 PR 后立即恢复原始 ruleset；即使合并失败也必须在 finally 恢复。读取线上配置并运行仓库 `manage_main_branch_ruleset.py verify` 确认恢复。
6. PR #504 合并最新 main 后重新执行全部场景，作为恢复后的 canary。全部 required checks 与 Windows/macOS 逐项 receipt 通过后合并 #504，记录结果并清理本任务 worktree。

## 当前证据边界

治理 PR [#505](https://github.com/BumStill/CodeFactory/pull/505) 已于 `2026-09-07T01:45:23Z` 合并，main commit 为 `af16e272e576ae8031181de8c1619d5c4d843428`。独立审查通过；本地 110 项 Python 回归、41 项 Skill Rust 测试、统一本地 Harness 与基线检查通过。其余五项 required checks 全绿后才执行更新；旧 gate 只报告 registry 与 validator 两个 trust-root 文件变化。

事务窗口从 `01:45:17.741949Z` 到 `01:45:26.750369Z`，约 9 秒。原始 ruleset `20222077` 已精确恢复，`manage_main_branch_ruleset.py verify` 返回 `status=converged`、`ruleset_matches=true`、无 classic review 漂移；六项检查仍绑定 GitHub Actions App 15368，strict/active/空 bypass actors 均保留。

PR [#504](https://github.com/BumStill/CodeFactory/pull/504) 已同步该主分支，作为恢复后全量 canary；最终通过与合并状态以该 PR 当前 head 的 Windows/macOS 执行回执及 required checks 为准。服务器根据完整 diff 生成的计划覆盖 27 个场景、111 个执行目标（Windows 102、macOS 9）；仅以 Cargo 全局变更做的预检为 110 个，本 PR 的具体改动还带入一个 Complex E2E 的额外目标。测试基础设施变更不单独发产品版本。11 个完整 E2E 的既有缺口仍保留。

## AI Collaboration

- context scope: registry、validator、Vitest include、Rust 条件编译、required workflow、ruleset 和匿名 CI 回执。
- assumptions: acceptance 源文件由现有 headless 入口执行；平台专用用例只在支持的平台上形成证据。
- review point: 独立审查 coverage 保留、平台映射、伪绿反例与临时门禁窗口。
- validation result: 以本 PR 的回归和审查输出为准；最终 canary 以 #504 新 head 的服务器结果为准。
