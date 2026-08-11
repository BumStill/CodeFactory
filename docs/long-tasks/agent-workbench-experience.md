# 现代 Agent Workbench 长任务记录

## Basics

- Task ID: CF-WB-20260730
- Title: Workspace 现代视觉与作业流演进
- Feature spec: `docs/specs/feature-specs/agent-workbench-experience.md`
- Related Req IDs: CF-WB-R1..R31

## Completion Standard

- Done means: CF-WB-R1..R31 均有实现与证据；PR+CI 合并；按刻意发版节奏进入公开安装产物；真实 CodeFactoryDev 与公开产物主路径通过。
- Blocked means: 同一外部阻塞连续三个 goal turn 无法推进，且已记录阻塞证据、责任人和下一步。

## Current State

- Current phase: 第二批已上线；后续无障碍与任务验收面收敛
- Current checkpoint: PR #362 已 squash merge 为 `defbc350`，Auto Release run `31489599245` 生成 v1.80.0，Release run `31490295360` 全绿；公开 DMG、Windows installer、updater 签名、`latest.json` 与匿名重新下载 DMG 的 released-artifact GUI 均已验收。该批统一控制面已 live。
- Next owner: 后续独立完成 R18 共用任务验收面、R30 本机 VoiceOver/200% zoom 证据及既有 P2 可访问性 backlog；browser child WebView 仍只按 Phase 1 同 URL 预览，EBP-R3/R9 保持 `not live`。
- Updated at: 2026-08-11

## Completed Items

- [x] 隔离 worktree，未触碰 main 上的按需浏览器 staged 改动。
- [x] 业务、架构、UX 与 feature spec 落库。
- [x] 明确外部机器人不属于产品。
- [x] 裁决结果 footer、release/live、token footer 和右侧 pane 冲突。
- [x] 先写视觉契约、结果、context、queue、sidebar、抽屉与 delivery 失败验收，并确认修复前失败。
- [x] 建立 light/dark surface、semantic status、15/13/11px 字号与 WCAG AA 对比度门禁。
- [x] 实现 880px 阅读列、统一 raised composer、真实 context 阈值、queue disclosure 与失败优先结果快照。
- [x] 实现会话搜索、完整标题、无嵌套交互结构，以及 ≤720px overlay 侧栏。
- [x] 实现任务/交付抽屉的 `aria-expanded`、初始聚焦、focus trap、Escape 关闭与回焦。
- [x] 固定区分 PR、CI、merge、formal release 和 live verification；Settings/Onboarding 不再把 release 写成上线。
- [x] 全量 89 个测试文件 / 447 项测试、TypeScript production build、治理基线与长任务结构 validator 通过。
- [x] 真实隔离 Tauri 在 800×600 浅色/深色完成成功与失败路径、两次 `update_plan`、结果持久化、抽屉键盘和无横向溢出验收。
- [x] 清理本任务启动的 app process group、1420/18765 端口、fixture server、测试 task row、wrapper 与隔离 HOME。
- [x] 第一批由 PR #270 合并，并随 v1.74.0 生成 macOS/Windows 安装产物与 `latest.json`。
- [x] 用户反馈的用量/context 位置由 PR #272 修正，并随 v1.75.1 发布。
- [x] PR #274 将侧栏收起控件归还“会话”栏头，并统一曲别针、单行文字与发送按钮的 32px 垂直基准；92 个测试文件 / 472 项测试、build、治理、远程 GUI 与本地真实 App 验收通过。
- [x] PR #274 所在批次随 v1.76.1 发布；macOS/Windows 构建、Windows Evolution 闭环、已安装 macOS 候选和重新下载的公开 DMG GUI 验收均通过。

## Remaining Items

- [ ] 补齐 1366×768 与 200% zoom 独立截图证据；≤720px 行为当前由真实 800×600 + `matchMedia` component test 覆盖。
- [x] 第二批：R17 单一右侧 pane arbiter，合并任务、Git、交付、证据和按需浏览器入口。
- [ ] 第二批：R18 Workspace/acceptance 共用 TaskActivityDrawer，并清理旧 TaskDashboard/ExecutionStream 虚假验收面。
- [x] 第二批：R19 顶栏图标优先并移除模型/思考/权限重复控件。
- [x] 第二批：R20 模型/思考/权限进入 composer，验证会话切换和下一回合生效语义。
- [x] 第二批：R21 context 圆环与累计 Token 渐进披露，不混合指标。
- [x] 第二批：R22 1440px dock、1024–1439px drawer、<1024px overlay、宽度调节与空白回收。
- [x] 第二批：R23 未知 context 不伪造百分比，累计 Token/成本/压缩信息只进入详情。
- [x] 第二批：R24/R25 Git、交付和任务图标化，异常/下一步保留短文字与完整可访问名称。
- [x] 第二批：R26 引入结构化下一动作责任人，纠正 `6/6 + failure evidence` 与 system-owned 恢复文案。
- [x] 第二批：R27 当前结构化状态与历史消息分层。
- [x] 第二批：R28 单 pane tabs、overlay 与 separator 完整键盘/focus 契约；原生 child WebView Escape 已桥接到宿主。
- [x] 第二批：R29 正文/composer 同一 880px 网格，多视口无页面级横向溢出。
- [ ] 第二批：R30 图标命中区、VoiceOver、200% zoom 和 reduced-motion 放行。
- [x] 第二批：R31 PR/CI/merge/release/public artifact/正式 App 完整上线证据。
- [ ] 无障碍 P2：补齐消息/工具/任务 disclosure 的 `aria-controls`，项目选择与会话菜单键盘模式，以及 ImagePreview 初始聚焦、focus trap、回焦和最小 11px 说明。
- [ ] 测试/性能 P2：清理 Workspace `act(...)` 警告，并单独治理 Vite 既存大 chunk 提示。

## Blockers

- None

## Evidence

- Local evidence:
  - 第二批：`pnpm test -- --run --reporter=dot` 为 101 files / 564 tests passed；`pnpm build` 通过。
  - 第二批 Rust：embedded browser 6/6、update_plan 5/5、plan hydration 1/1、legacy/intermediate schema repair 1/1、agent-loop owner wire 1/1。
  - 第二批治理：repo governance baseline 与 long-task validator 均通过；`git diff --check` 通过。
  - PR #362：最新 head 的 `governance-baseline`、`agent-bridge-linux`、`check-frontend`、`check-rust` 与 `remote-real-app-gui` 全部通过，PR 为 `MERGEABLE/CLEAN` 且相对 `origin/main` 无落后。
  - 锁屏无关桌面证据：GitHub macOS runner 从 PR #362 精确 head 构建并启动真实 debug App，窗口状态 `ok`，尺寸 `1024×674`，截图 `1136×786`；截图可见模型入口已进入 composer footer、顶栏无旧模型/思考/权限重复控件。该证据证明 Tauri 壳与候选渲染，不冒充未执行的本机 VoiceOver、200% zoom 或 child WebView 接管路径。
  - 本机边界：CodeFactoryDev 读取被 macOS 锁屏拒绝；未请求绕过系统锁。R30 的 44px、accessible name、focus 与 overlay 契约有 component/TypeScript/build 证据；VoiceOver/200% 本机交互保持未验证，发布后由锁屏无关 headless/远端 release artifact 分层证据承接，不能互相冒充。
  - 第二批 browser 边界：child WebView 只提供 lease 初始 URL 的独立 Phase-1 预览，与 Agent 的 `LOCAL` ChromiumDriver 不共享 Cookie、DOM、导航或控制权；不得宣称实时观察/接管。
  - `pnpm test`：89 files / 447 tests passed。
  - `pnpm build`：TypeScript + Vite production build passed。
  - `python3 tools/governance/validate_repo_governance_baseline.py`：pass。
  - `python3 tools/governance/validate_long_task_record.py --task-record-path docs/long-tasks/agent-workbench-experience.md`：pass。
  - 真实隔离 Tauri：固定本地 fixture endpoint 下，两步计划从 `completed/in_progress` 演进到 `completed/completed`，两次 `update_plan` 均为 `done`，切换会话后结果快照仍存在。
  - 第一批真实边界路径：OpenRouter 不可用时曾显示“需要处理”；本批按 R26 进一步区分 system-owned 恢复、证据待复核与明确用户动作。交付 remote unavailable 不伪造 PR/CI/release/live。
  - 可访问性树：任务与交付抽屉 Escape 关闭后焦点分别返回触发按钮。
- Release evidence:
  - v1.74.0 与 v1.75.1 均为公开非预发布 release，包含 macOS/Windows 安装产物、签名和 `latest.json`。
  - v1.76.1 为公开非预发布 release；macOS DMG、Windows NSIS、签名与 `latest.json` 均可公开下载，HTTP 返回 200。
  - `latest.json` 的版本为 `1.76.1`，Windows x86_64 / NSIS 与 Darwin arm64 URL 均指向 v1.76.1 且签名非空。
  - Release run 30690692975 全部绿色；公开 DMG 被重新下载并通过安装产物与 GUI 验收。
  - 第二批由 PR #362（merge `defbc350`）进入 main；最终 squash footer 保留 `Release-Urgency: immediate`。发布规划器对 `v1.79.2..main` 返回 `slot=minor`、`immediate=1`、`hold=0`、`invalid_urgency=0`。
  - Auto Release run `31489599245` 通过仅含四个版本 manifest 的 PR #363，merge/tag SHA 为 `4c2fd733`；Release run `31490295360` 的 changelog、prepare、Windows Evolution closed loop、macOS 安装产物 GUI、finalize/publish 和匿名公开 DMG GUI 复验全部通过。
  - v1.80.0 为公开非 Draft、非 prerelease latest release，精确包含 macOS DMG、Windows NSIS、Windows `.sig`、macOS updater archive、macOS `.sig` 与 `latest.json` 六个资产。`latest.json` 版本为 `1.80.0`，三个平台 URL 均指向 v1.80.0 且签名非空。
  - 发布后公开 DMG SHA-256 为 `177b855ad8c1ceb32e5e8d130558f020cce083c023ddfc0e1ef6a865c46a2a27`，与 GitHub asset digest 一致，`hdiutil verify` 通过；published release receipt 的 `app_version=1.80.0`、`build_git_sha=4c2fd733…`、`status=pass`，窗口 `1024×674`、proof tier 为 `published-release-artifact-gui`。
  - macOS 分发保持仓库现行兼容通道：未使用 Apple Developer ID/公证，严格 `codesign --verify` 不作为已满足声明；Tauri updater 签名与首启 Gatekeeper 边界按 README 和 `macos-release-trust` 规格如实保留。
- Blocking evidence: 当前无。

## AI Collaboration

- context scope: Workspace theme、session rail、conversation、result、composer、context、task activity、delivery、Settings/Onboarding delivery copy。
- assumptions: 第一批不修改持久化 schema；release snapshot 当前没有 live verifier 字段；外部机器人不属于产品。
- review point: 独立 QA 先后阻止低对比 muted/status 色、嵌套交互、抽屉焦点、窄屏、9–10px、hardcoded 状态色和 reduced-motion 缺口进入交付；逐项修复后复审。
- validation result: 第一批与两轮用户反馈均已公开发布；第二批 R17、R19–R29 已由 PR #362 合并并随 v1.80.0 上线，PR/CI、远端真实 App、双平台构建、公开元数据与 released-artifact GUI 均通过。R18、R30 的本机 VoiceOver/200% zoom 与既有 P2 backlog 仍明确留在后续，不冒充本次已完成。

## Stop Boundary

- 不在单元测试或 build 后停止。
- 不在 PR、CI 或正式 release artifact 后把未验证 live 写成上线。
- 只有完成全部证据，或达到有证据的阻塞条件时停止。
