## M107 — Req ID 与验收
- R1/R5：规格 `docs/specs/feature-specs/m107-reconcile-replay-real-incidents.md` 保留任务需求并记录 M98/M103/M106 的假设、遗漏与对账边界。
- R2d：`m107_normal_committed_steps_are_not_recovery_failures` 十二步 SQLite 回放先红（Applied(1) != Applied(0)）后绿。
- R2a–c/R3：`m107_replay_finished_regression_error_continues`、`m107_replay_finished_python_get_error_continues`、`m107_replay_waiting_local_edit_continues` 检查真实工具恢复存储的本地失败/等待形状；临时目录用例先红（unknown != reconciled）后绿。`m103_current_owner_reconciles_effect_free_request_at_serial_boundary` 与 `m103_reconciliation_closes_prior_episode_before_revision_resume` 覆盖第四现场及 revision 接管；`m103_reconciliation_refuses_unproven_side_effect` 保留无回执副作用停止反例。
- R2e：`forge_read_only_queries_are_probes_not_checks` 新增 gh run view && git show && git fetch 链，现有 M52b 分类已经正确；根因是 agent-loop 每个工具批次之后主动生成完成拒绝。删除该分支，最终响应的完成门禁保持。`m107_tool_steps_do_not_request_completion_recovery` 验证三步工具正常执行，不强迫下一轮工具恢复。
- R4：E2E-001/E2E-007 automated_by 登记真实测试，已有 PR Cargo / nightly workspace 执行，保留 partially_implemented，不声称完整桌面 E2E 已实现。

## Self-acceptance
| 验收项 | 结果 | 证据 |
| --- | --- | --- |
| 正常十二步骤无恢复失败 | PASS | SQLite 回放 |
| 已知 error、本机临时目录输出继续 | PASS | tool_recovery 回放 |
| Python GET error 继续 | PASS | tool_recovery 回放 |
| 本地编辑 waiting 继续 | PASS | tool_recovery snapshot/hash 回放 |
| in_flight / 旧 episode 对账后继续且预算不变 | PASS | provider_auth_recovery |
| 真正未决副作用不重放 | PASS | provider_auth_recovery 安全反例 |
| 只读复合链非必过检查 | PASS | agent-core 分类测试 |
| 工具步骤不触发完成拒绝 | PASS | agent-loop scripted transport |

## Verification
- Rust：`CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-m107" cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast` PASS。
- Frontend：`pnpm test` PASS，151 files / 1013 tests（含 lightModeAudit）；`pnpm build` PASS。
- Governance baseline PASS；scenario/readme validators 在交付前执行。
- 无 UI 修改，不需要浏览器截图。真机验收由编排方发版后完成；本单只合并，不发版，not live。

## Applicable Harnesses / AI Collaboration
Spec（需求与追溯）、Compatibility（旧 revision / checkpoint）、Observation（终态与预算断言）、Payload（tool result 与回执）、Release（合并交接）、AI Collaboration。
Context scope: provider store/transport boundary、tool recovery、agent-loop、M52b classifier。Assumptions: 已结束 error 是已知失败而非成功；本地快照不能证明未知外部写成功。Review point: permit/revision/generation 围栏保持。Validation result: 全量 Rust/前端测试与构建通过。任务模板禁止 delegate_tasks，且恢复边界为共享 writer，本单单执行流。

## Spec Amendment
无。限制：四个现场由合成形状及现有 production store 集成测试组成，不复制生产内容，不写生产 DB；新增独立命名中的第四现场复用已存在 M103 回放。本单未执行安装版真机验收，编排方发版后逐项核对规格末节。

Scenario-Test: HLT-001, HLT-002, HLT-005
README-Update: reviewed
README-Update-Reason: 内部恢复与完成证据判定修复，不改变公开安装、隐私、平台或产品承诺。
