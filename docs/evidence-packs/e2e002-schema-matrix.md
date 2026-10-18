# E2E-002 schema matrix 验收与交付证据

## R1–R4 / Acceptance

| Req | 测试与证据 | 结果 |
| --- | --- | --- |
| R1 | 生产迁移前缀 1/6/10/19，真实 SQLite ledger 和 provider 表边界断言 | pass |
| R2 | 正式 binary 在源码目录外运行；四行 schema_matrix；历史 session/root/Objective 身份、重开幂等、零副作用与无额外人工 prompt | pass |
| R3 | binary 现有 PR/nightly 硬门禁强制运行矩阵；新增 rust:historical_schema_matrix_preserves_identity；registry 保留 partially_implemented/L3 | local pass，CI 待交付 |
| R4 | 先 missing old-schema matrix，再 identity-free history must not create a new task 红灯，同一测试修复后绿灯 | pass |

前端 `pnpm test`：151 files / 1013 tests（含 lightModeAudit）；`pnpm build` 通过。
Rust focused：`cargo test --manifest-path src-tauri/Cargo.toml --test history_schema_matrix -- --nocapture` 1 passed。
完整 workspace 相同命令重跑通过（lib 1671 passed / 10 ignored；其余 integration/crates/doc tests 全绿）。首轮既有 socket accept 竞态 `WouldBlock`，按模板仅重跑一次即通过，未修改无关 socket 代码。
治理 `validate_scenario_test_governance.py --ci`、README validator、repo baseline 均 OK；历史 smoke Node 合同 5/5。新增 registry 目标为 `rust:historical_schema_migration_prefixes_preserve_sessions`，正式 binary 集成测试继续实际运行，不以静态合同代替行为验收。

## 实现与边界

- 修复 sqlx 迁移宏：相对于 crate 的 `../migrations` 指向根目录旧 1–5；`./migrations` 才是任务指定 1–20。兼容 checksum mismatch 的生产 ensure_schema fallback 保留。
- schema 1 无可证明历史任务身份时，中文明确拒绝，事务回滚；6 将旧 root 绑定统一 Objective；10/19 保留已存在的固定匿名 Objective。
- 合成 fixture 全部临时生成并清理，不读取生产数据库。
- 信任根 registry 修改由任务明确授权；cannot self-modify 由编排方处理，不弱化 validator/workflow。

## Applicable Harnesses / AI Collaboration

Spec：原文规格和 Req 映射。Compatibility：真实旧 SQLite 前缀、ledger。Observation：正式 binary receipt 字段与清理断言。AI Collaboration：context scope 为匿名迁移与 admission；assumptions 为 L2 不等于 L3；review point 为旧身份不可猜测与事务回滚、迁移目录；validation result 为外部 binary 和回归测试。遵循用户模板单执行流、不调用 delegate_tasks，不声称独立 QA。

## 真实应用验收 / not live

真机验收由编排方发版后完成：隔离合成旧数据库，选择历史会话发送继续，核对同一身份、即时进度、重启后无重复任务和副作用；无身份旧历史显示明确中文终态。不修改 UI，不声称 L3/installed artifact 已验收；本任务只交付到 merged，不发版。

Spec Amendment: 无待决需求修改。

Scenario-Test: ALL
README-Update: reviewed
README-Update-Reason: 历史数据库兼容矩阵与安全续接未改变 README 公开平台、安装或隐私承诺。
