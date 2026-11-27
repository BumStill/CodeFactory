# M283 完成证据

## Basics

- Task ID: M283
- Title: 权限身份对齐失败与重启救援接续
- Feature spec: `docs/specs/feature-specs/m283-permission-align-stall-restart-orphan.md`
- Related Req IDs: R1–R6
- PR: #1075（当前仍为 draft）

## Completion Standard

- Done means: R1–R6 的实现、先红后绿真实入口、受影响场景、Rust 编译、治理门禁全部通过；PR 正文含逐项证据并转 ready 后按用户授权交付。
- Blocked means: 真实浏览器或正式产物因当前执行环境不可运行，且已尝试受管替代路径并记录精确外部阻塞。

## Current State

- Current phase: 本地实现完成，正在执行最终场景与治理门禁。
- Current checkpoint: split、权限审计、有界唤醒、Git rescue、主动停止回归均绿；正式产品仍为 not live。
- Next owner: CodeFactory agent。
- Updated at: 2026-11-25

## Completed Items

- [x] 复现 split 清理错误改绑 live working 回合。
- [x] 在任何 worktree 重建、快照、合并与退休前保护 live executor。
- [x] settled 回合迁移 SQL 排除 working + no terminal reason。
- [x] 权限 admission 保留一次就地重试，耗尽后登记 bounded continuation wake。
- [x] 权限失败写入 secret-free `objective_events`。
- [x] workspace rescue 成功后把 `WaitingWake` 投影持久化。
- [x] durable cancel intent 阻止系统恢复创建 wake。
- [x] 记录 R1–R6 traceability 与 Scenario-Test 声明。

## Remaining Items

- [ ] 完成权限 UI 受管浏览器门禁。
- [ ] 重跑长任务记录与治理校验。
- [ ] 提交并推送草稿进度。
- [ ] 更新 PR 正文 R1–R6 表，转 ready，再按授权交付。
- [ ] 编排方发版后执行正式产物真机验收（本次本地交付不冒充 live）。

## Blockers

- 系统 Chrome headless 首次执行被 macOS sandbox 拒绝访问 Chrome Crashpad/Mach rendezvous；产品断言未执行。正在改用 CodeFactory 受管浏览器作为独立替代路径。

## Evidence

### 范围与假设

- Context scope：权限 identity admission、split workspace 修复、重启后 workspace rescue、thin continuation。
- Assumptions：用户主动停止和已交付事实优先；权限 scope 与 heredoc 安全规则不可放宽。
- Review point：所有“系统自动重试/自愈”文案必须有在途重试或持久 wake；救援成功必须更新持久 thin projection。
- Validation result：本地 Rust/SQLite 自动化通过；正式发布产物真机验收尚未执行，当前 not live。

### 先红后绿

| 行为 | 红灯 | 修复 | 绿灯 |
| --- | --- | --- | --- |
| split 退休保护 | SQL 回归中 working 回合实际改绑 keeper，退出 1；Rust 真实入口进入快照/退休路径 | split 修复在所有副作用前检测 live executor；迁移 SQL 排除 live working 回合 | M283 Rust split 测试 + SQL 补充回归通过 |
| 结构化失败审计 | 新测试发现 `detail_json` 原样持久化凭据形态，退出 101 | 使用 `trajectory::redact_text` 后写事件 | 权限审计测试通过 |
| 有界唤醒 | 原失败分支仅有文案，没有持久 wake | admission 就地重试后复用 M282 continuation | 权限入口和 continuation 测试断言 pending wake |
| 救援接续 | 真实 Git rescue 后仍为 `Ended/orphaned_by_restart`，退出 101 | 持久化 continuation 返回的 `WaitingWake` | M161 Git rescue 测试通过，断言状态 + pending wake |
| 主动停止 | 规格要求停止优先 | wake 子系统创建前检查 durable cancel intent | 停止反例通过且不创建 wake table |

### 已通过命令

- `pnpm cargo:shared -- test --manifest-path src-tauri/Cargo.toml m283_ -- --nocapture`：4 passed。
- `python3 tools/governance/test_m283_split_retirement.py`：1 passed。
- `pnpm cargo:shared -- test --manifest-path src-tauri/Cargo.toml m161_dirty_incident_is_rescued_and_continues -- --nocapture`：1 passed。
- `pnpm cargo:shared -- check --manifest-path src-tauri/Cargo.toml`：通过。
- `python3 tools/governance/validate_scenario_test_governance.py`：通过。
- `python3 tools/governance/validate_repo_governance_baseline.py`：通过。

### R1–R6

| R | 是否完成 | 证据 |
| --- | --- | --- |
| R1 | 是 | M179 current objective rebind + M283 split live executor 真实 workspace 保护 + SQL settled/live 矩阵 |
| R2 | 是 | 一次就地重试；失败后 pending bounded wake |
| R3 | 是 | 真实 Git rescue 从 `orphaned_by_restart` 转为持久 `WaitingWake` |
| R4 | 是 | secret-free `objective_events` 字段级断言 |
| R5 | 是 | objective 切换、失败 wake、救援非终态、主动停止反例均有行为断言 |
| R6 | 是 | 未修改 permission classifier、auto-allow、shell policy 或 heredoc parser |

### 场景声明

- `Scenario-Test: HLT-001, HLT-004, UI-009, RTE-004, E2E-009`
- Release-Urgency：默认。
- Live 状态：**not live**。

## AI Collaboration

- context scope: 同 Evidence 范围。
- assumptions: 不放宽权限或 heredoc；停止与交付事实优先。
- review point: live executor 零副作用保护、secret-free 审计、真实 wake、救援持久投影。
- validation result: Rust/SQLite 行为与治理基线通过；UI 受管浏览器与远端交付待完成。

## Stop Boundary

- 全部本地验收通过前仅允许 git push 保存草稿进度，不调用交付工具。
- 本地验收完成后必须更新 PR 正文、转 ready，并使用用户已授权的默认 Release-Urgency 交付。
- 正式产物真机验收由编排方发版后独立记录；本地完成不得表述为 live。
