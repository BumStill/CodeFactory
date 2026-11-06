# M249：额度测试偶发失败，拖垮了一次发版

## 现场（2026-11-06）
- 版本 PR #1004 的 `check-rust`（run 42202790088，Windows）失败：`agent::failover::tests::m164_reserve_capped_primary_waits_when_every_alternative_is_blocked`，`left: None, right: Some(...)`，断言是 `unavailable.resume_at_ms`。
- 这导致 Auto Release run 42202711401 失败，v1.96.0 晚了约 45 分钟。同样的代码在 main 上 CI 是绿的。

## 推测（未证实）
测试写的是全局 `shared_quota_ledger()`，以 `base_url` 为键。其它并行测试可能用同样的键写入或 `clear`。实现前先证明。

## 规格
- R1：确定性复现这次失败，把根因写进规格。如果推测不对，按实际根因修。
- R2：所有读写全局额度账本的测试互相隔离，隔离方式由实现决定。
- R3：如果根因是产品代码的并发问题，修产品，不修测试。
- R4：不隔离或跳过测试，不放宽断言，不加固定等待。不改信任根。

## 测试
- 确定性复现用例：修复前失败，修复后通过。
- 这组测试在本机并行连跑 20 次全过，PR 写明命令和结果；Windows 以 PR 的 `check-rust` 为准。

## 交付
授权交付。PR body 写 `Scenario-Test`，按任务模板收尾。

## Implementation Notes

R2 的“互相隔离”指状态隔离；R4 的“不隔离或跳过测试”指不得把失败测试排除出正常测试执行。

### Requirements Traceability
| Req ID | 最低证据 |
| --- | --- |
| R1 | 确定性并发复现与原 resume_at_ms 断言 |
| R2 | 同键跨线程写入、清理不污染其他测试；完整并行回归 |
| R3 | 检查产品账本锁与测试时钟的区别；不无依据改产品行为 |
| R4 | 无 ignore、无放宽断言、无固定等待、无信任根修改 |

### Applicable Harnesses
Spec Harness、AI Collaboration Harness；Windows 兼容性由 PR check-rust 验证。本任务不发布、不改 UI；真机验收由编排方发版后完成。

### 测试矩阵
确定性状态竞争回归；原 M164 准入测试；额度与 failover 相关测试并行 20 轮；Rust workspace 全量；前端全量与 build；场景及 README 治理。
