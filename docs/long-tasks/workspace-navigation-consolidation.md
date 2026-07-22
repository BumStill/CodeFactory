# Workspace 顶栏与 Welcome 收敛长任务记录

## Basics

- Task ID: CF-NAV-20260722
- Feature spec: `docs/specs/feature-specs/workspace-navigation-consolidation.md`
- Related Req IDs: CF-NAV-R1..R8

## Current State

- Current phase: Implementation — tests and browser acceptance first
- Current checkpoint: 已确认 v1.58.1 未覆盖顶部代码；此前顶部整合没有形成 PR，repository-owned specifications 停在 Draft #159。当前交付分支已并入 #159 实现，开始统一收敛顶栏与 Welcome。
- Next owner: 当前 Codex 完成 red/green、真实 App、PR+CI、合并与刻意发版。
- Updated at: 2026-07-22

## Completion Standard

CF-NAV-R1..R8 全部有证据；repository-owned specifications 与本改造在同一 main 基线上；真实 Tauri 和公开产物均验证后才可标记 live。

## AI Collaboration

- context scope: Workspace header、App view routing、Settings tabs、Welcome、token usage acceptance、repository-owned specifications。
- assumptions: 设置作为唯一全局入口；当前会话的 Git/检查点仍是高频操作；Welcome 不承担完整分析。
- review point: 只读 UX 审查指出当前 4×7 邮票热力图、重复标签、大片空白、微缩状态字符和中英文混排均需结构性修复。
- validation result: 实现、前端单测、headless viewport 验收、真实 Tauri 主路径和构建均已通过；等待 PR、CI、合并、发布产物验证。

## Completed Items

- [x] CF-NAV-R1..R8 的业务、架构、UX 设计和 feature spec 已落库。
- [x] 独立测试先失败后通过，覆盖顶栏边界、设置能力入口、Welcome 用量摘要和 repository-owned specifications。
- [x] Workspace 顶栏只保留会话级高频动作，低频全局能力统一进入设置。
- [x] Welcome 改为紧凑中文首屏；28 天趋势使用同一蓝色色相，以高度和深浅表达用量，无使用日不再显示虚线框。
- [x] Settings 保留完整日历热力图及能力入口。
- [x] 真实 Tauri 完成 Workspace、Welcome、设置功能页、资源中心主路径验证。
- [x] 1366x768、800x600/700、375x812 headless viewport 验收通过且无页面横向溢出。

## Remaining Items

- [ ] 提交当前分支并创建 PR。
- [ ] PR CI 全绿后合并到 `main`。
- [ ] 按刻意发版流程切出版本，并验证公开安装产物的真实主路径。
- [ ] 回填 PR、CI、release 和公开产物证据。

## Blockers

- 当前无实现或验证 blocker。

## Evidence

- Frontend: `pnpm test` — 63 files / 270 tests passed。
- Build: `pnpm build` passed。
- Usage viewport acceptance: `pnpm test:usage:headless` passed，覆盖 1366x768、800x600、375x812。
- Repository intent acceptance: `pnpm test:repository-intent:headless` passed，覆盖 1366x768、800x700。
- Real app: `/Applications/CodeFactoryNavigationDev.app` 完成 Workspace 顶栏、Welcome、设置功能页和资源中心验证。
- Governance: `python3 tools/governance/validate_repo_governance_baseline.py` passed。

## Stop Boundary

只有 PR 合并、刻意发版完成且公开安装产物通过真实主路径验证后，才允许把本任务标记为完成或 live；在此之前只能报告对应阶段的已验证状态。
