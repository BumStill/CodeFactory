# CodeFactory 仓库协作规则（AGENTS）

## 最高原则（适用本仓库与任何未来仓库）
- **持续合并，刻意发版**：合并 ≠ 发版。发布管线**不由 push/merge 触发**；版本在需要时按需（`workflow_dispatch`）或每日（`schedule`）**成批**切出，把"自上个 tag 起的所有合并"汇成一个版本，且**仅当存在 `feat`/`fix` 时才发**（`chore`/`ci`/`docs`/`refactor`/`test` 搭下一次 feat/fix 一起发）。完整规则见 `docs/principles/release-cadence.md`，这是跨仓最高原则，可原样复制到任何仓库。

## 入口顺序
- 日常任务先读 `docs/repo-quick-profile.md`、当前任务说明和 quick gate 结果。
- 只有 quick gate 标记 release、compatibility、observation、payload、viewport 或 governance-change 风险时，才加载 `docs/repo-governance-profile.md` 和全局治理参考。
- 本仓库采用 `codex-delivery-governance`，全局标准入口为 `multi-project-harness-standard.md`。

## 全局治理源
- `codex-delivery-governance`: `C:/Users/yourz/.codex/skills/codex-delivery-governance/SKILL.md`
- `multi-project-harness-standard.md`: `C:/Users/yourz/.codex/skills/codex-delivery-governance/references/multi-project-harness-standard.md`
- `governance-change-control.md`: `C:/Users/yourz/.codex/skills/codex-delivery-governance/references/governance-change-control.md`
- 仓库本地只能适配 surface、主路径、命令、平台、证据和产品约束；不得在本仓库重定义全局 Harness 分类、对象模型、选择规则、最低证据或 validator contract。

## 文档语言
- 新建或大幅修改的项目文档、规格、计划、长任务记录、证据包和治理记录默认使用中文。
- 代码标识、命令、API 字段、日志、错误信息和第三方接口名保留英文。

## 角色门禁
- 规划 / 系统工程角色：整理规格、Requirements Traceability、Primary User Path、测试矩阵和 Applicable Harnesses。
- 开发角色：只按已批准规格实现，按 Req ID 记录变更，保持最小 patch。
- QA / 验收角色：独立验证真实用户路径，有权拒绝仅靠结构检查、HTTP 200、非空数组或 mock 成功的完成声明。
- 发布 / 运维角色：仅在 release-facing 任务中启用，负责部署、配置、回滚和 live verification。
- 非平凡产品、代码、发布、治理变更，在工具环境允许且用户未关闭时默认启动真实后台角色 sub-agent。允许跳过的情况：只读或极小变更、工具不可用、用户要求单执行流、安全审批未完成、任务无法安全拆分。跳过时最终输出必须写明原因。

## Harness 范围
- 所有实现或治理任务至少启用 Spec Harness。
- release-facing 或生产可见任务同时启用 Compatibility Harness、Release Harness、Observation Harness。
- 上传、文件、音视频、请求体、网关限制、对象存储相关任务追加 Payload Harness。
- 移动端、首屏、固定操作区、溢出、动画或响应式布局相关任务追加 Viewport Harness。
- AI 生成代码、多角色协作、治理修改或关键假设不清时追加 AI Collaboration Harness。

## CodeFactory 主路径
- Primary User Path: 用户打开 CodeFactory，选择项目工作目录和模型，输入编程任务，模型读取项目、提出或执行受控工具调用，用户审批高风险操作，系统展示 diff、命令输出、测试结果和会话记录。
- release-facing 缺陷默认必须完成本地修复、相关测试、发布交接或部署、真实主路径验证；如果不能发布，最终输出必须明确写 `not live`、阻塞原因和下一步命令或人工动作。

## 证据最低线
- 不得单独把 UI 可见、HTTP `200`、mock 通过、非空数组、行数正确、deploy 命令成功或本地测试通过当成完成证据。
- 工具、文件、命令、OpenRouter、SQLite、安装包和发布路径的变更必须记录实际处理 route、字段级断言或主路径证据。
- AI Collaboration 最小记录：context scope、assumptions、review point、validation result。

## 并行分支同步门禁
- 多分支或多 agent 并行开发时，动手前、提交前、push/开 PR 前都必须 `git fetch --prune origin main`，确认当前分支已包含最新 `origin/main`；没有包含时先 `git merge origin/main`，解决冲突并重新验证后再继续。
- 本仓库使用版本化 pre-commit hook：`.githooks/pre-commit` 调用 `tools/git/ensure_branch_current.py`，会在提交前 fetch 默认分支并阻止未合并最新默认分支的提交。
- 首次使用或 checkout 后运行：`git config core.hooksPath .githooks`。如果 hook 拦截，按提示 merge 最新默认分支，不要用重复 PR 或旧基线继续开发。
- 只有用户明确批准紧急热修时才允许 `CODEFACTORY_SKIP_SYNC_GATE=1 git commit ...`，最终说明必须标记 `hotfix bypass` 并补回 PR+CI。

## Worktree 生命周期与 Cargo 缓存
- 创建或 checkout 新 worktree 后，版本化 `post-checkout` hook 会把缺失的 `src-tauri/target` 链接到共同缓存；已有本地 target 一律不自动替换。
- PR 通过 GitHub squash 合并后，执行者必须从其他 checkout 运行 `pnpm worktrees:closeout -- --path <自己的绝对路径> --apply`，由 GitHub 已合并 PR 判定后删除目录和本地分支；不得用 `merge-base` 否定 squash 合并。
- 长会话优先使用 `pnpm cargo:shared -- <cargo arguments>`；裸 Cargo 在新 worktree 中也必须落到共同 target，不得形成新的独占 `src-tauri/target`。

## 验证与测试
- 行为或代码修改前必须先写独立测试或可执行验收，并先看到失败。
- 修改后运行相关测试；若存在 lint、typecheck、build，应优先补齐并执行。
- 未运行验证命令时，不得声称完成，只能说明已修改并待验证。
- 本仓库治理基线验证命令：`python tools/governance/validate_repo_governance_baseline.py`。

### 会话静默禁止规则（硬规则）

**适用范围：** 任何耗时 >30 秒且不产生实时输出的 bash 调用（CI 轮询、cargo build、
Cargo test、Evolution smoke、桌面编译等）。

**硬性要求：**
- 发起耗时命令前，必须告诉用户「这一步约 X 分钟，在做什么」。
- 命令完成后立即报告结果；若预计还要等，给出选择「继续等 vs 先做别的」。
- **禁止连续的 >60 秒无输出工具调用。** 一次耗时 bash 结束后必须回到对话层
  报告进度，确认方向正确再进入下一步。
- **失败即报告。** 工具调用出错时，第一时间用自然语言告诉用户：
  出了什么错、现在在尝试怎么修。禁止连续 >2 次错误尝试不汇报。
- **同一策略不过三。** 同一条修复路径连败两次后，第三次必须向用户报告
  局面和选项，不停下闷头继续试。

**历史教训（必须避免重蹈）：**
本会话中修复一个 Markdown 表格渲染和删除一个按钮，用户打断了 5 次——每次都
因为 agent 在轮询 CI 或修复分支冲突时静默超过 60 秒。用户看到的是「思考中」，
实际是死循环或翻车抢救。**静默 = 用户以为你挂了。**

### UX 行为变更必须实地验证（硬规则）

**适用范围：** 任何会改变用户感知行为的 frontend 修改——
滚动、聚焦、动画、布局、输入处理、拖拽、粘贴、键盘行为、stream 渲染、
focus trap、modal 行为、文件附件流。

**硬性要求：**
- **不得仅靠 vitest / jsdom / 单元测试** 就声称完成。jsdom 不渲染 CSS、
  不计算 layout、不处理真实滚动；通过的 unit test 完全可能对应一个
  实际不工作的界面。
- 必须在 `pnpm tauri dev` 或 `pnpm dev` 启动的真实 app 里执行
  Primary User Path 中受影响的步骤，至少包含一种成功路径和一种边界
  路径（如：自动滚动需要测「正常 stream 时尾巴跟随」+「我向上翻时
  不要被强拉回」）。
- PR 描述必须列出**实地走过的具体场景**，不能只写「测试通过」。
- 修复 UX 回归（如 stick-to-bottom）时，必须先在真实 app 中复现 bug，
  把复现路径写进 PR，再修，再用同样路径验证修复。
- 实地无法验证（如 Windows 限定行为在 Mac 上）必须在 PR 写明哪些
  场景没本地验证 + 为什么 + 替代证据（截图、日志、视频）。

**历史教训（必须避免重蹈）：**
stick-to-bottom 滚动行为从 v0.3.7 到 v0.3.20 反复修了 6 次，每次都
有 vitest 回归测试通过，但实际 app 里依然有断裂场景。原因：测试只
覆盖了 hook 的内部状态，没人在 app 里真正流式输出 + 翻阅 + 等待
re-pin。**单元测试通过 ≠ 用户体验正确**。

**macOS 上的 dev binary 实地验证路径：**
Tauri dev 二进制不在系统应用注册表里，`computer-use.request_access`
默认找不到。仓库提供 `scripts/install-dev-app-wrapper.sh`：
- 一次性运行：在 `/Applications/CodeFactoryDev.app` 装一个 wrapper
  bundle，shim 调用 `pnpm tauri dev`，自带 ad-hoc 签名 + 注册 LaunchServices
- 装好后 agent 可用 `request_access(["CodeFactoryDev"])` 拿到 `tier:"full"`
  权限，screenshot/click/type/scroll 全部可用
- 后续 UX 验证都以此为准；不再有"Mac dev binary 不能 live verify"借口

**在 worktree 里实地验证：改指针，不要重装 wrapper。**
wrapper 在**每次启动时**解析要跑哪个 checkout，顺序为
`$CODEFACTORY_DEV_TARGET` → 指针文件 `~/.codefactory/dev-app-target` →
安装时所在的 checkout。所以 worktree 只需要改一行指针，bundle 不动：

- 验证前：`scripts/install-dev-app-wrapper.sh --target`（不带参数即当前目录）
- 验证后：`scripts/install-dev-app-wrapper.sh --clear-target` 交还主 checkout
- 随时确认当前指向：`scripts/install-dev-app-wrapper.sh --show`
- `/tmp/CodeFactoryDev-<YYYYMMDD>.log` 每次启动都会写
  `target checkout: <path> [via ...]` 和 commit；截图取证时一并附上这两行，
  否则无法证明截的是本次改动的代码

**禁止**在 worktree 里跑无参数安装：那会把 bundle 的兜底路径固化成一个
close out 之后就消失的目录，等于把用户的 wrapper 弄坏。指针失效时 shim 会
自动回落到安装时的 checkout 并在日志写 `warn:`，但兜底本身必须是长期存在的
主 checkout。`--target` 对旧版 bundle 会直接报错（旧 shim 不读指针，写了也
只会拿错 checkout 的证据），按提示从主 checkout 重装一次即可。

**窗口落在副屏会截不到图。** 已实测 `computer-use` 对副屏窗口截图可能直接
失败（SCContentFilter 返回 nil）。wrapper 默认在启动时把主窗口钉在主屏
`60,60`（`CODEFACTORY_DEV_WINDOW_ORIGIN="x,y"` 改坐标，`off` 交还 Tauri）。
若窗口仍出现在副屏或截图失败：先用 `computer-use.switch_display` 切到该屏，
不行就把窗口拖回主屏再截；两者都不行时按下面的降级条款在 PR 写明。

如果出现新的 harness 限制（不是这个），可以临时降级为：
- 在 PR description 写明 **"agent live verification not feasible"** + 具体原因
- 改写**针对真实失败模式的单元测试**（不是 happy path）
- 列出 **scenarios needing manual verification** 让用户在 release 上回填
- **同时**：立刻把这个限制当成下一个 PR 的工程问题来解决，沉淀脚本/工具到仓库

## 规格与长任务
- 长期存在的业务能力、架构约束、主路径、兼容语义和验收基线必须写入 `docs/specs/`，不能只存在于一次性计划。
- 长任务使用 `docs/long-tasks/` 记录；`long tasks` 只能在完成或有证据阻塞时停止。
- `docs/planning-agent.md`、`docs/development-agent.md`、`docs/qa-acceptance-agent.md`、`docs/release-ops-agent.md` 是角色协作入口。

## governed CI/CD
- `.github/workflows/governance-baseline.yml` 负责在 PR/push 上检查治理基线文件。
- `.github/workflows/governed-delivery.yml` 是手动触发的治理交付入口。
- 改 CI、发布配置、生产配置、schema、依赖或删除数据前必须先说明风险。
- 发布节奏遵循 `docs/principles/release-cadence.md`（持续合并、刻意发版、非 feat/fix 不单独发版）；`auto-release.yml` 是其参考实现。

Repository: `CodeFactory`

<!-- AI-CODING-OS:BEGIN project-codefactory-agents -->
# AI Coding OS Project Profile - CodeFactory

Generated preview. Promote edits through OpenClaw AI Coding OS proposals.

## Project

- id: `codefactory`
- path: `/Users/leo/Projects/CodeFactory`
- kind: `ai-coding-product`
- primary executors: codex, claude-code

## Durable Notes

- Local assistant with reusable knowledge, office connectors, controlled delivery.
- Keep dirty main checkout untouched; use sibling worktrees for risky slices.
- Do not treat release/CI/health as complete until real artifact or app behavior is verified.

## Required Behavior

- Check current git/runtime/test state before relying on memory.
- Keep user changes untouched unless explicitly asked to modify or revert them.
- Record durable lessons as AI Coding OS proposals with evidence.
- Separate business/product reasoning from implementation detail when reporting back.
<!-- AI-CODING-OS:END project-codefactory-agents -->
