# 会话执行路线与结果快照长任务记录

## Basics

- Task ID: CF-CCE-P1-P3-20260728
- Title: 结构化执行路线、结果快照与可解释时间估算
- Feature spec: `docs/specs/feature-specs/chat-continuity-conversational-evidence.md`
- Related Req IDs: CF-CCE-R10、CF-CCE-R13、CF-CCE-R17、CF-CCE-R19..R22

## Completion Standard

- Done means: P1 执行路线、P2 结果快照、P3 时间估算通过失败先行测试、1000-event 门槛、四视口/主题和真实 Dev App 主路径，并完成 PR/CI/main。
- Blocked means: 同一外部阻塞有连续证据，且本地、headless 与 GitHub runner 均无安全替代。

## Current State

- Current phase: P1–P3 实现、真实 App 验收和完整自动化完成，进入 PR/CI 门禁。
- Current checkpoint: P0 已由 PR #235 合并；P1–P3 已完成 plan event、结果快照、证据化重新总结和有来源估时。
- Next owner: Codex 当前任务。
- Updated at: 2026-07-28

## Completed Items

- [x] 审计 MessageList、chat reducer、tool event、history paging 和任务状态接口。
- [x] 确认计划、结果和估时不能靠前端猜测或模型自由文本。
- [x] 补齐 CF-CCE-R19..R22 与 P0–P3 映射。
- [x] 先写失败测试，再实现 `update_plan`、append-only 持久化、stream/hydration 同构和时间样本查询。
- [x] 实现紧凑进度、等待/计划变化、结果快照、结果/过程切换和本地证据化重新总结。
- [x] 1000 次计划事件和 1000 项工具证据保持有界；低层 `update_plan` 工具卡不污染会话时间线。
- [x] 在真实 Dev App 中完成 4 阶段只读主路径，并覆盖等待原因、12 秒外部等待、会话切换恢复。
- [x] 浅色/深色的 800×600 与 1366×768 结果卡均完成实机检查。
- [x] 完成前端 371 项、Rust workspace 741 项、生产构建、治理 validator 和 diff 检查。
- [x] 确认匿名/headless 路径不暴露 `update_plan`，不会留下匿名计划记录或触发不可用工具。

## Remaining Items

- [ ] 完成 PR/CI/main，并由 closeout 工具清理本次 worktree 与分支。

## Blockers

- None

## Evidence

- Local evidence:
  - 真实任务运行中依次显示 `0/4`、`1/4 (25%)`、`2/4 (50%)`、`3/4`，当前步骤与下一步均可见，百分比明确来自 4 个计划步骤。
  - 运行中未出现“展开较早的执行过程”；等待阶段显示“正在等待验收计时器”。
  - 终态结果卡在同一次渲染中出现，显示 `4/4`、3 项操作、44.1s；会话切换后仍恢复相同数据。
  - 证据化重新总结输出“完成 4/4 个计划步骤；修改 0 个文件；执行 0 项验证；没有失败证据。”，未发起模型请求。
  - 结果视图恢复等待历史并显示“没有失败操作证据”。
  - 800×600、1366×768，浅色与深色四种组合均可读且无结果卡横向溢出。
  - 精确 800×600 运行态显示 `2/4`、`50%`、`来自 4 个计划步骤`、当前“窄屏计时器二”和下一步“全部完成并一句总结”，五项同屏可见。
  - `pnpm test -- --run`：82 个文件、371 项通过；`pnpm build` 通过。
  - `pnpm cargo:shared test --manifest-path src-tauri/Cargo.toml --workspace`：741 项通过、0 失败、6 项既有 ignored。
  - 两个新增 Rust 模块 scoped rustfmt、长任务 validator、治理 baseline validator 和 `git diff --check` 通过。
  - 全仓 `cargo fmt --check` 仍会被既有未格式化的 pptx、shell policy、xlsx 等无关文件拦截；未把全仓机械格式噪音混入本次改动。
- Release evidence: `not live`
- Blocking evidence: none

## AI Collaboration

- context scope: chat plan event、tool evidence、MessageList、history hydration、task_runs 与 timing profile。
- assumptions: 计划只属于当前 turn，不成为永久工具栏；估时样本少于 3 不展示；重新总结不二次调用模型。
- review point: 计划变化审计、百分比来源、估时样本、千级事件有界性、结果与完整过程切换。
- validation result: 真实 App 主路径、边界视口、全量自动化和治理校验已通过；PR/CI/main 待完成。

## Stop Boundary

- 不在组件测试、本地 build 或 PR 创建后停止。
- 只有 P1–P3 合并并完成真实 App 主路径，或有明确 blocker，才允许停止。
