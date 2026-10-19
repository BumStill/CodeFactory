# thin 等唤醒 P0 验收记录

## 范围与决策
按 `task-simplify-phase1-p0-wait-gate.md` 实现 R1–R5；不改 legacy，不操作生产数据库。R6 不迁移旧误终态，保留原 CF-BG 完成入队链；真实安装版恢复核对由编排方发版后完成。

## TDD 与 Requirements Traceability
| Req | 用例 / 证据 | 结果 |
| --- | --- | --- |
| R1/R2/R5 | `thin_wait_gate_five_rounds_never_exhausts_technical_failures`：缺监视、定时唤醒、真实 sleep 后台进程三分支，各五次等待 | 修复前失败 `waiting is not a technical failure`；修复后定时与缺监视分支通过，最终进程分支见 PR 全测结果 |
| R1 | 有覆盖时失败签名、remediation、next_observation_at 均为空，状态 waiting_system | 字段级断言 |
| R2/R5 | `thin_three_identical_failures_end_with_summary_without_legacy_budget` | 真实 adapter_error 三次结束对照通过 |
| R3 | `a_finished_task_persists_every_field_and_wakes_with_exit_code_and_tail`、`a_due_wakeup_wakes_the_session_and_is_not_fired_early` | 原 CF-BG 真进程 / 定时入队测试，workspace 通过 |
| R4 | `thin_explicit_lower_ceiling_reports_authorized_merge_requirement` | 先失败 effective_ceiling 缺失；修复后通过，断言 armed 与授权说明 |
| R6 | 保持现有 send_message 入队，不迁移生产数据 | 旧误终态恢复未作真机结论 |

## 命令证据
- `CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-p0-wait-gate" CARGO_INCREMENTAL=0 cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast`：首次修复后主库 1760 passed / 10 ignored，其他 workspace 与集成测试通过。最终进程分支添加后同命令再次全跑，最终结果写 PR。
- `pnpm test`：152 文件、1017 测试通过，含 lightModeAudit；`pnpm build` 通过。
- Python 3.11 执行统一 local scenario gate、governance baseline、`validate_scenario_test_governance.py --ci`、README validator 均通过。
- 运行 route：`ObjectiveStore::apply_decision → bound_system_recovery → thin wake_source_cover`；只有 thin + 完成证据提示进入该分流，真实技术错误仍走连续三次判断。
- 交付 route：先检查会话授权与请求越权，再 `delivery_thin::execute` 使用已解析授权；merged 保持幂等 armed，不直接合并。

## Harness 与 AI Collaboration
Spec：原文入库。Compatibility：legacy 路径不变、无 schema 迁移。Observation：covered 等待独立原因、无技术失败签名。Release：只合并，not live。AI Collaboration：按模板单执行流，不调用 delegate_tasks；context scope 为 P0/thin 等待/交付，assumptions 为复用旧状态承载，review point 为正常等待与真实错误隔离、授权不降级，validation result 如上。

## 编排方发版后真机验收
连续五次等待仍等唤醒；后台结束自动续跑；merged 授权与较低请求仍确保标签；核对旧误终态后台完成能否生成后续回合。本任务不发版、不操作生产 DB。无 Spec Amendment。
