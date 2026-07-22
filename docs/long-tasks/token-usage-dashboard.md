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

- Current phase: Phase 4 — Release Proof
- Current checkpoint: Phase 0-3 的首发范围已实现并通过本地回归、双视口 headless 与隔离 Dev App 数据库验证；尚未完成 PR/CI、合并、刻意发版和精确公开产物 smoke，因此仍为 `not live`
- Next owner: release/QA 角色复核 diff 与证据，完成同步门禁、PR+CI、合并、按需发版和 macOS/Windows 精确产物验证
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

## Remaining Items

### Phase 4 — Release Proof

- PR、CI、独立 QA、合并和刻意发版。
- Windows installer、macOS DMG、公开产物下载、build metadata、迁移、多轮、地图、重启和 rollback smoke。

### 后续增强（非本次首发门禁）

- 模型、Endpoint、项目多维筛选及自定义起止日期。
- 跨版本采集覆盖率 SLO、系统级通知和团队/跨设备账单。

## Blockers

- 当前无 blocker。
- 本机锁屏时不要求用户解锁、不绕过 macOS 安全；继续运行 headless 双视口、CLI 和 GitHub runner。Tauri 壳由远端 macOS artifact smoke 补充，不能用 headless 冒充。

## Evidence

- Local evidence: `pnpm test` 62 files / 266 tests 全绿；`pnpm test:rust:fast` 430 passed / 6 ignored；`pnpm build`、`cargo check`、治理基线、长任务 validator 通过。
- Local evidence: `pnpm test:usage:headless` 在 1366×768 与 375×812 通过，验证今日摘要、三种地图指标、zero/missing/today/over-budget、日期下钻、真实作业日志交接、预算与移动端按月列表；证据目录为系统临时目录 `codefactory-token-usage-headless`。
- Local evidence: `/Applications/CodeFactoryUsageDev.app` 使用隔离 HOME/DB 启动；settings 为 `full_access=true, ask=[], deny=[]`；SQLite 存在四张用量/预算/迁移表，`usage-v1` 回执存在且 `PRAGMA integrity_check=ok`。
- Release evidence: 尚无，本能力当前 `not live`。
- Blocking evidence: 本机交互桌面在锁屏状态不可见；未要求用户解锁，已用 headless 双视口和隔离 Dev runtime 继续验证，Tauri 壳由发布 runner/产物 smoke 补齐。

## AI Collaboration

- context scope: `src-tauri/src/agent/mod.rs`、`commands/costs.rs`、provider Usage、SQLite、`ContextUsageBar`、`CostDashboardSection`、Welcome、Settings、Profile 和现有作业日志 route。
- assumptions: 首版本地单用户、逐 Provider 请求计量、Token 预算优先、无云端账单依赖；成本与任务质量分离。
- review point: development 不得先画地图再补数据；QA 必须用工具多轮真实路径对账；release 必须验证 exact artifact 和旧库迁移。
- validation result: 实现与本地验证完成；PR/CI、合并、release 和精确产物验证待完成，因此当前 `not live`。

## Stop Boundary

- 不在 schema 或单元测试通过后停止。
- 不在地图出现、Dev App 通过、PR 合并或 CI 绿色后停止。
- 不把 headless 当作 Tauri 壳或发布安装包证据。
- 只在 Done 标准全部满足，或达到有证据的 Blocked 标准时停止。
