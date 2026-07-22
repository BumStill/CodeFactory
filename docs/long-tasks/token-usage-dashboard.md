# Token 用量、消耗地图与预算长任务记录

## Basics

- Task ID: CF-USAGE-20260722
- Title: 每日 Token 用量、GitHub 风格消耗地图与预算
- Feature spec: `docs/specs/feature-specs/token-usage-dashboard.md`
- Related Req IDs: CF-USAGE-R1..R22

## Completion Standard

- Done means: R1-R22 全部落地；逐 Provider 请求数据真相、当地日、成本语义、兼容迁移、新会话摘要、年度/28 天地图、拆分与日志深链、预算提醒、anonymous/隐私边界均通过；PR+CI 合并；刻意发版；Windows 安装包和 macOS 公开 DMG 的精确可执行主路径验证完成。
- Blocked means: 同一外部权限或环境条件连续三轮阻断且已穷尽 headless、CLI、GitHub runner 和隔离数据目录等安全替代路径，或用户明确暂停。锁屏本身不是 blocker。

## Current State

- Current phase: Complete — Live in v1.58.0
- Current checkpoint: CF-USAGE-R1..R22 已由 PR #160 合并；PR 与合并后 main CI、锁屏无关远端 GUI、刻意发版、Windows/macOS 构建、公开 DMG 二次下载与 GUI smoke 均通过，能力已随 v1.58.0 上线
- Next owner: 产品运营按真实使用数据观察采集缺失率、预算触发与高消耗来源；后续增强不属于本次完成门禁
- Updated at: 2026-07-22

## Completed Items

- 明确产品采用新会话摘要、设置详情和 Workspace 底栏三级结构。
- 明确 GitHub 风格 Token 消耗地图是设置核心视图，新会话展示最近 28 天缩略图。
- 完成行业参照与本地产品取舍：Usage/Cost 分离、产品内阈值提醒、逐 Provider 响应计量。
- 审计现有实现并确认：`record_cost_entry` 位于最终无工具轮、UTC 截日、统一默认价格、Profile 成本透视不适合作为目标真相面。
- 定义逐请求 Usage、成本语义、当地日、anonymous、幂等、迁移、深链、隐私和 Observation 合同。
- 建立 CF-USAGE-R1..R22、Primary User Path、Applicable Harnesses、测试矩阵和 Evidence Pack。
- 新增逐 Provider round 的 `model_usage_events` 真相表、`attempt_id` 幂等键、当地日聚合、来源/缺失状态与 additive 历史回填。
- 新增 `usage_migration_receipts` 和 `usage_budget_receipts`，分别保留迁移计数与日/月 50%/80%/100% exact-once 提醒回执。
- Welcome 显示今日 Token/请求/成本语义/预算或 7 日均值及 28 天缩略图；anonymous 明确不进入持久统计。
- Settings 新增一级「用量与预算」，含 today/7d/30d 摘要、90d/半年/一年地图、Tokens/预算占比/请求次数、zero/missing/today/over-budget 状态和窄屏按月列表。
- 日期下钻展示入口与高消耗会话；“查看会话”和“查看作业日志”分离，只有能由 `task_runs` 反查父会话时才显式展示真实作业日志入口并高亮任务。
- Workspace 底栏切到新真相源；Profile 移除旧的第二套成本统计写入口。
- 文档复核将模型/Endpoint/项目筛选和自定义日期明确列为后续分析增强，不冒充本次首发能力。
- PR #160 通过全部检查后 squash 合并到 `main`；合并后精确提交的 CI 与治理门禁再次通过。
- Auto Release 将版本从 v1.57.0 升至 v1.58.0；Windows 安装器、macOS DMG、Tauri updater 资产与跨平台 `latest.json` 已公开。
- 发布 workflow 在独立 macOS runner 上安装构建产物，并在发布后从匿名公开 URL 重新下载 DMG 完成二次 GUI 验证。
- 本次隔离 Dev 验收结束后已主动关闭 Tauri/Vite/esbuild/debug 进程组并确认 1424 端口释放；后续 Dev 实例必须按工作树登记并在用完后清理。

## Remaining Items

### 后续增强（非本次首发门禁）

- 模型、Endpoint、项目多维筛选及自定义起止日期。
- 跨版本采集覆盖率 SLO、系统级通知和团队/跨设备账单。

## Blockers

- 当前无 blocker。
- 本机锁屏未要求用户解锁、未绕过 macOS 安全；headless 双视口、隔离 runtime、远端真实 GUI 和公开 macOS artifact smoke 共同补齐了验证链路。

## Evidence

- Local evidence: `pnpm test` 63 files / 267 tests 全绿；`pnpm test:rust:fast` 430 passed / 6 ignored；`pnpm build`、`cargo check`、治理基线、长任务 validator 通过。
- Local evidence: `pnpm test:usage:headless` 在 1366×768 与 375×812 通过，验证今日摘要、三种地图指标、zero/missing/today/over-budget、日期下钻、真实作业日志交接、预算与移动端按月列表；证据目录为系统临时目录 `codefactory-token-usage-headless`。
- Local evidence: `/Applications/CodeFactoryUsageDev.app` 使用隔离 HOME/DB 启动；settings 为 `full_access=true, ask=[], deny=[]`；SQLite 存在四张用量/预算/迁移表，`usage-v1` 回执存在且 `PRAGMA integrity_check=ok`。
- PR evidence: PR #160 合并提交 `0da9525`；PR CI run `29908680308`、远端真实 GUI run `29908680360`、合并后 main CI run `29909157635` 全部成功。
- Release evidence: Auto Release run `29909721966` 生成 v1.58.0 与版本提交 `10bbf34`；Release run `29909741236` 的 changelog、prepare、Windows、macOS、finalize 与 published-macOS 六个 job 全部成功。
- Public evidence: GitHub Release `v1.58.0` 为非 draft、非 prerelease；DMG、Windows EXE、双平台 updater 包/签名和 `latest.json` 均已上传。匿名 DMG range 请求返回 HTTP 206；manifest 版本为 1.58.0，并包含 `darwin-aarch64`、`windows-x86_64`、`windows-x86_64-nsis`。
- Blocking evidence: 本机交互桌面在锁屏状态不可见；未要求用户解锁，最终由 lock-independent remote GUI、安装后 artifact smoke 与公开产物二次下载验证补齐，不再构成 blocker。

## AI Collaboration

- context scope: `src-tauri/src/agent/mod.rs`、`commands/costs.rs`、provider Usage、SQLite、`ContextUsageBar`、`CostDashboardSection`、Welcome、Settings、Profile 和现有作业日志 route。
- assumptions: 首版本地单用户、逐 Provider 请求计量、Token 预算优先、无云端账单依赖；成本与任务质量分离。
- review point: development 不得先画地图再补数据；QA 必须用工具多轮真实路径对账；release 必须验证 exact artifact 和旧库迁移。
- validation result: 实现、本地验证、PR/CI、合并、刻意发版和精确公开产物验证全部完成；v1.58.0 当前为 `live`。

## Stop Boundary

- 不在 schema 或单元测试通过后停止。
- 不在地图出现、Dev App 通过、PR 合并或 CI 绿色后停止。
- 不把 headless 当作 Tauri 壳或发布安装包证据。
- 只在 Done 标准全部满足，或达到有证据的 Blocked 标准时停止。
