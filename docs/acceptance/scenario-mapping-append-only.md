# M165 场景映射只增不删验收账本

规格：`docs/specs/feature-specs/scenario-mapping-append-only.md`

| 需求 | 状态 | 证据 |
| --- | --- | --- |
| R1 非信任根文件只允许新增路径到既有场景的映射 | done | `docs/testing/scenario-mappings.json` 为独立映射数据；`tests/test_scenario_mappings.py::ScenarioMappingsTests::test_addition_union_and_execution_plan` 断言新增映射生效且该文件不属于 TRUST_ROOT_FILES；`::test_delete_shrink_and_retarget_rejected` 断言删除、缩小 glob、改指向及删除整个文件均被拒绝。场景定义仍存于 registry。 |
| R2 相对 base 只增不减、active 引用与危险 glob 校验 | done | `tools/governance/validate_scenario_test_governance.py::load_scenario_mappings` 比较 base 与候选的精确映射二元组；`tests/test_scenario_mappings.py::ScenarioMappingsTests::test_delete_shrink_and_retarget_rejected`、`::test_unknown_inactive_broad_and_trust_root_rejected`、`::test_malformed_symlink_and_bad_base_fail_closed` 覆盖删改、未知及非 active 场景、整仓与信任根 glob、坏 JSON、符号链接和不可解析 base；`tools/governance/run_scenario_harness_gate.py::main` 将 base_ref 传入加载入口。 |
| R3 两处映射取并集且 M156 自动填写同步使用 | done | `tests/test_scenario_mappings.py::ScenarioMappingsTests::test_addition_union_and_execution_plan` 通过合并加载后的 registry 断言影响集、声明校验及 build_execution_plan 的 scenario_ids；`::test_m156_adapter_uses_union` 调用真实 `scripts/delivery-scenario-body.py::reconcile`，断言新产品文件自动生成正确 Scenario-Test。 |
| R4 新增通过、非法变更拒绝且 registry 仍受信任根保护 | done | `tests/test_scenario_mappings.py` 六项回归：`test_addition_union_and_execution_plan`、`test_delete_shrink_and_retarget_rejected`、`test_unknown_inactive_broad_and_trust_root_rejected`、`test_malformed_symlink_and_bad_base_fail_closed`、`test_addition_not_self_modify_but_registry_still_is`、`test_m156_adapter_uses_union`；实施阶段先观察新增加载接口及映射覆盖失败再修复；其中信任树测试断言只加 mappings 不触发 self-modify，改 registry 字节仍触发 cannot self-modify。 |
| R5 不迁移既有 registry 映射 | done | `tests/test_scenario_mappings.py::ScenarioMappingsTests::test_addition_union_and_execution_plan` 对比加载前后 registry 文件内容不变；`git diff --exit-code origin/main -- docs/testing/scenario-registry.json` 验证本 PR 没有修改 registry。新映射仅写入 `docs/testing/scenario-mappings.json`。 |

## 本次补账本的验证

- `python3.14 tools/governance/validate_acceptance_ledger.py --repo . --base-ref origin/main`：补齐前已复现缺少本账本并列出 R1–R5；补齐后须返回 pass。
- `python3.14 -m unittest discover -s tests -p 'test_scenario_mappings.py' -v`：执行上述六项行为回归。
- `python3.14 tools/governance/validate_repo_governance_baseline.py --base-ref origin/main`：验证 M162 接入后的治理基线。

## 交付与真实应用验收分工

本次仅补账本并推送同一 PR #857，不发版。`scenario-gate-pr` 的 cannot self-modify 由编排方按已授权流程处理，本账本不将该远端检查记为通过。真机验收由编排方发版安装后执行：新增产品文件及映射，核对自动填写的 Scenario-Test 和场景执行回执；删去已有映射应被拒绝。无新增 Spec Amendment；按任务模板采用单执行流，不启动子 agent。
