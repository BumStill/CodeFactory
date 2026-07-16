# CodeFactory 自托管开发待办

## Basics
- Task ID: CF-LT-SELF-HOSTED-DEV
- Title: 使用 CodeFactory 持续开发 CodeFactory
- Feature spec: 复用具体改动对应的 `docs/specs/feature-specs/*.md`，不新增自开发产品功能
- Related Req IDs: 由每个具体产品改动的规格决定

## Completion Standard
- Done means: CodeFactory 能作为开发工具打开自身仓库，按既有规格完成拆解、实现、测试、真实 App 验证、PR/CI、合并、发布和发布后复验；本记录中的流程缺口均已关闭或被更具体的长期任务接管。
- Blocked means: 同一外部条件连续三轮阻止当前产品改动，且没有其他可执行待办或安全替代验证路径；必须记录分支、工作目录、失败证据和恢复动作。

## Current State
- Current phase: 基本能力确认完成，可以开始自托管开发；本轮不实现新的“自开发闭环”产品功能。
- Current checkpoint: v1.45.3 已具备自主模式、规范优先拆解、任务 DAG、acceptance criteria、Agent 工具调用、任务内重试、项目验证、checkpoint/revert、per-task worktree、失败归因、Evidence Pack 和配置化 draft PR。headless、CLI 与 GitHub macOS runner 已证明锁屏不阻断验证和发布。
- Next owner: 后续开发直接在 CodeFactory 中打开 `/Users/leo/Projects/CodeFactory`，从本记录或具体 long task 领取一个产品待办，按“一项改动 -> 验证 -> PR/CI -> 合并 -> 发布 -> 发布版复验”执行。
- Updated at: 2026-07-16

## Completed Items
- 审计 `origin/main` v1.45.3 的自主任务、scheduler、worktree、checkpoint、verification、failure attribution、draft PR 和锁屏交付路径。
- 确认 CodeFactory 作为开发工具已经具备开发自身仓库的基本能力，不需要先开发一个新的自闭环产品模块。
- 确认已有 `docs/long-tasks/terminal-bench-21-evaluation.md` 和 `docs/long-tasks/evolution-agent-closed-loop.md` 可继续作为具体产品改动的权威待办，不在本记录复制其实现项。

## Remaining Items
- [ ] 每次只从具体 long task 领取一个可独立发布的产品改动，并先同步最新 `origin/main`。
- [ ] 使用独立 clean worktree 开发 CodeFactory 自身，避免本地 dirty main 或旧 worktree 污染结果。
- [ ] 在 CodeFactory 自主模式中带入明确目标、验收条件和对应 Req ID；不允许只有宽泛聊天目标。
- [ ] 每个确定性改动完成独立测试、真实 App 主路径、PR/CI、合并、刻意发版和发布版复验，不在本地候选阶段停止。
- [ ] 应用或开发进程中断后，重新打开同一项目并核对 task、branch、checkpoint 和验证证据；当前普通 `task_runs` 没有自动恢复死亡 `running` owner 的合同，不能假定已自动续跑。
- [ ] 如果单任务失败，使用现有 failure attribution 和 `修复可修复项` 重新执行；provider/credential/runtime 外部问题先修复条件，不盲目重复消耗。
- [ ] 继续 `docs/long-tasks/terminal-bench-21-evaluation.md` 当前 P0：减少发布版 `build-cython-ext` canary 的重复安装/扫描，保留最终 install/runtime/tests 时间预算，并处理 transport failure 后的可恢复续跑。
- [ ] 上述通用修复必须先通过非 benchmark CodeFactory 产品任务，再走 PR/CI、合并和发布；随后用同一发布 tag 重跑 canary，再决定是否启动固定 18 题。
- [ ] 远端无开放 PR 时，审计残留分支是否已被主干等价覆盖；已覆盖分支清理，不把过期实现重新合入并回退当前产品。

## Blockers
- None。当前 CodeFactory 可以开始自托管开发；缺口是流程注意事项和具体产品待办，不是启动前阻塞。

## Evidence
- Local evidence: `src/pages/Workspace/WorkspacePage.tsx` 已有自主模式和规范优先路径；`src-tauri/src/agent/scheduler.rs` 已有 retry/acceptance/verification；`src-tauri/src/agent/worktree.rs` 已有任务隔离；`src-tauri/src/commands/tasks.rs` 已有 checkpoint、Evidence Pack 和 draft PR 入口。
- Release evidence: v1.45.3 已完成公开 macOS artifact GUI 验证，验证和交付不依赖本机解锁。
- Blocking evidence: `src-tauri/src/storage/tasks.rs` 当前没有普通 `task_runs` 的进程 owner/lease 启动恢复；这是已知限制，但不阻止在 CodeFactory 中开始下一项开发。

## AI Collaboration
- context scope: origin/main v1.45.3 的 Workspace、自主任务、scheduler、worktree、task storage、Evolution、Git/PR 和 release evidence。
- assumptions: “自闭环”指用 CodeFactory 作为开发工具持续完成自身产品改动，不是新增一个面向用户的自修改产品能力。
- review point: 先复用已有工具能力和 long task；只有实际开发被稳定缺口阻断时，才把缺口升级为产品需求。
- validation result: 基本开发能力具备；当前未实现项已记录，本轮不继续开发功能。

## Stop Boundary
- 本记录只管理自托管开发方式与未完成项，不代替具体产品规格。
- 不把本地测试或分支存在当成已产品化；每个确定性改动必须合并并按发布节奏交付。
- 不把已被主干等价覆盖的旧分支再次硬合并。

