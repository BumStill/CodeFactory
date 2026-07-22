# 仓库归属规范与会话内计划实施记录

## Basics
- Task ID: CF-LT-REPO-SPECS
- Title: 退役独立 Specs 产品模块并把长期规范归还代码库
- Feature spec: `docs/specs/feature-specs/repository-owned-specifications.md`
- Related Req IDs: CF-REPO-SPECS-R1..R7

## Completion Standard
- Done means: 独立规范/计划入口与 runtime 写入链路已删除；CodeFactory 自身旧规范完成 Git 迁移；Agent 能在会话中发现仓库权威并继续自动委派；相关测试、真实 App、PR/CI、合并及刻意发版边界都有证据。
- Blocked means: 同一外部条件连续三轮阻断交付，且没有安全的本地、headless 或 GitHub runner 替代路径；必须记录失败证据和恢复动作。

## Current State
- Current phase: 堆叠 draft PR 已创建，等待上游与 CI 门禁。
- Current checkpoint: PR #159（`codex/repo-owned-specs` → `codex/resume-completion-recovery-tdd`）已推送；两条 governance checks 通过。
- Next owner: 当前 Codex 主执行线。
- Updated at: 2026-07-22

## Completed Items
- [x] 同步 `origin/main` 并确认 PR #157 是会话内拆任务的现有实现线，未重复实现。
- [x] 在独立 sibling worktree `/Users/leo/Projects/CodeFactory-repo-owned-specs` 开发。
- [x] 建立 feature spec、business design、architecture design、UX design。
- [x] 先取得 Workspace、Agent prompt、Control Plane 三组失败测试，再完成实现。
- [x] 抽离通用 one-shot AI transport，删除 SpecsPage/store/IPC 与 Issue→Spec 入口。
- [x] 将两份已跟踪旧规范原字节迁入 `docs/specs/feature-specs/`，并更新治理文档。
- [x] 完成前端、Rust、构建、治理与真实浏览器验收。

## Remaining Items
- [ ] 等待上游 #157 修绿并合并；把 PR #159 retarget 到 `main`，触发并通过完整 CI。
- [ ] 合并后按 release cadence 判断刻意发版，并在发布产物复验。

## Blockers
- 上游 PR #157 当前 `check` 失败；本切片可继续开发和验证，但在 #157 修绿/合并前不能进入 main。
- `.github/workflows/ci.yml` 只监听 base=`main` 的 PR；PR #159 作为堆叠 PR 当前只运行 governance。上游合并并 retarget 后才会触发完整 CI，不把当前状态标为 CI green。

## Evidence
- Local evidence:
  - `pnpm test`: 59 files / 254 tests passed。
  - `pnpm test:rust:fast --lib`: 425 passed / 0 failed / 6 ignored。
  - `pnpm exec tsc --noEmit`、`pnpm build`、`python3 tools/governance/validate_repo_governance_baseline.py`、`git diff --check`: passed。
  - `pnpm test:repository-intent:headless`: Google Chrome 实际渲染通过；覆盖 1366×768、配置最小宽度 800×700、会话执行详情与远程 Issue 详情。
  - 桌面 App 已成功启动并检查首页顶栏；项目选择已定位到本 worktree，但 macOS 在最终 `Open` 前锁屏，因此项目会话用 lock-safe 浏览器门禁补证，不声称完成了锁屏后的桌面点击路径。
  - 旧规范 SHA-256 保持不变：`settings-hooks-remotes-tabs.md` 为 `5c02ff36687b57fa066fe1eb02935f0f0535c168734646c34bb2dbe8d9fb7449`；`token-cost-dashboard.md` 为 `45d92bd60f47fa2989a40c60d4cfcd7840adc69c0fd1e682aa580a19ee29b67d`。
- Release evidence: 当前 `not live`。
- PR evidence: draft PR #159；governance-baseline 2/2 passed；完整 CI 未触发。
- Blocking evidence: PR #157 `mergeStateStatus=UNSTABLE`，Windows `check` 失败；其他 governance、agent bridge、remote GUI checks 通过。

## AI Collaboration
- context scope: Workspace、SpecsPage/store、Tauri specs commands、Remote Git、Control Plane、Agent prompt、task/evidence compatibility 和 repo docs。
- assumptions: `docs/specs`/`docs/design` 是 CodeFactory 仓库当前约定；其他仓库由自身 `AGENTS.md` 和已有结构决定，产品不强制创建同名目录。
- review point: Specs 模块混入通用 one-shot transport，必须先解耦；历史 task/evidence 字段必须保留。
- validation result: 规划/架构/QA 确认的完整退役范围已实现；历史 task/evidence provenance 与兼容枚举保留；全量本地门禁和 lock-safe 真实浏览器验收通过。

## Stop Boundary
- 不在只删除图标、只通过单元测试或只创建 PR 时停止。
- 未合并、未刻意发版或未完成发布产物复验时必须明确写 `not live`。
- 上游 #157 阻塞时继续完成所有可独立验证，不把并行分支问题包装成功能完成。
