# M256：Windows CI 偶发失败家族审查

## 现场（2026-11-05 到 11-06）
Windows 的 `check-rust` 接连偶发失败，每次都拖住合并或发版：
- `browser_open_reads_the_same_content_when_repeated`（M242，已修）
- `m164_reserve_capped_primary_waits_when_every_alternative_is_blocked`（M249，已修）
- `m202_pending_failure_subscription_survives_restart`：`left: Running, right: WaitingWake`，`thin_derive.rs:1980`
- `delivery::scenario_tests` 5 个用例同时超过 45 秒 worker deadline，日志为空（`delivery_scenario_tests.rs:944`）

## 规格（放到 `docs/specs/feature-specs/m256-windows-ci-flake-family.md`）
- R1：统计最近 30 天 `main` 和 PR 上 `check-rust` 的失败：测试名、次数、耗时，以及重跑后是否通过。写进规格，按共同原因分组。
- R2：给每组找根因。优先看共享全局状态、固定时长等待、并行进程数和 Windows 启动成本。
- R3：修排名靠前的组。至少修上面的 m202 和 delivery worker deadline。如果是产品缺陷，修产品。
- R4：worker 超时时，失败信息必须说清卡在哪一步，不能是空日志。
- R5：不隔离或跳过测试，不放宽断言，不靠单纯加大超时来变绿。确实要调整等待时要有测量依据。不改信任根。

## 测试
- 每个修复都有确定性复现：修复前失败，修复后通过。
- PR 写明本机连跑的命令和结果。Windows 以 PR 的 `check-rust` 为准，本机不能代替。

## 交付
授权交付。PR body 写 `Scenario-Test`，按任务模板收尾。
