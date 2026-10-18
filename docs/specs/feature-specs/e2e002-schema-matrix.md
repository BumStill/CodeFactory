# E2E-002 L2：旧 schema 迁移矩阵（历史会话简短继续，跨版本数据库）

## 背景
- 2.0 门禁③：docs/testing/scenario-registry.json 中 E2E-002「历史 session 简短继续精确续接」为 partially_implemented，remaining_gaps 之一是「旧 schema fixture 迁移矩阵仍需扩展」。另一条（L3 真实桌面 WebView）不在本单范围。
- 动手前读 registry 中 E2E-002 定义、现有 `--history-session-smoke` 实现与其 fixture、src-tauri/migrations/ 下的迁移序列、docs/long-tasks/scenario-test-completion.md。

## 需求
- R1 用合成数据为若干具有代表性的历史 schema 版本（至少：最早仍受支持的版本、引入 objectives 统一控制面之前、引入 provider episode/attempt 之前、当前上一版）各生成一个最小匿名 SQLite fixture（只含形状，不含真实内容），由迁移脚本或测试代码生成，避免提交大二进制。
- R2 对每个 fixture：打开 → 自动迁移 → 选中一个历史会话发送「继续」→ 断言同一 session/objective 身份被正确续接（或在确实不可续接的旧形状上给出明确、用户看得懂的终态），无重复副作用，无需人工 prompt。
- R3 把矩阵接入现有 PR 或 nightly 门禁（照 registry execution 字段），满足 validate_scenario_test_governance.py 的 hard-gate 要求；registry 修改本单明确授权：E2E-002 automated_by 增加新目标、remaining_gaps 移除已完成的 L2 条目，保留 L3。scenario-gate-pr 的 cannot self-modify 由编排方处理。
- R4 先失败；在长任务记录追加进展。

## 约束
- 合成 fixture，不使用、不复制任何真实用户数据库；不写生产 DB。

## Implementation Notes

- 迁移边界：1（最早受支持的初始表）、6（统一 objectives 前）、10（provider episode/attempt 前）、19（当前 20 的上一版）。使用内嵌生产迁移的前缀生成真实 `_sqlx_migrations` 和 SQLite，关闭后通过生产 `storage::db::connect` 打开迁移。
- schema 6 之后使用迁移 3 自带的最小历史 segment/turn；10/19 另保留固定匿名 Objective，并补当时由运行时维护的 objective_id 关联列。不从当前完整 schema 删除列伪造历史。
- schema 1 没有可证明的历史执行身份；简短继续必须显示明确中文错误并原子回滚，不创建新任务冒充续接。
- 已有 binary smoke 的 PR/nightly/release 调用强制运行矩阵；集成测试也通过正式 binary 执行同一入口。不修改 workflow/validator。

## Requirements Traceability / 测试矩阵

| Req | 可执行证据 |
| --- | --- |
| R1 | 每个 fixture 迁移前 ledger 精确等于边界，provider 表存在性符合边界；全为匿名常量 |
| R2 | 正式 binary 运行四版本矩阵；会话/根/Objective、重复打开、重试拒绝无副作用、用户消息数及 receipt 数断言 |
| R3 | 新 Rust integration target + 原 binary PR/nightly 硬门禁；registry 保留 partially_implemented 和 L3 |
| R4 | Node 失败优先合同和正式 binary 行为失败优先；长任务进展 |

## Applicable Harnesses

Spec、Compatibility、Observation、AI Collaboration；本单不发版、不变 UI，不声明 L3 或 installed artifact 已验收。

## 真实应用验收分工

真机验收由编排方发版后完成：在隔离合成旧数据库上安装新版，选择历史会话发送「继续」，核对同一 session/Objective、即时进度、重启后身份与无重复副作用；无执行身份的最早版本显示明确终态。不得接触真实用户数据库。
