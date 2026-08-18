# 场景测试

本目录的权威机器源是 `scenario-registry.json`。它统一管理业务场景、UI acceptance、运行时 smoke、复杂 E2E 组合、证据等级和执行 gate。

## 当前规模

- 18 个去重后的逻辑 Scenario；
- 6 个复杂真实 E2E 组合设计；
- PR、nightly、release 对同一场景的重复执行只算不同证据层，不重复计数。

`gates` 表示当前真实 workflow 覆盖，不表示目标。存在脚本但未接入 CI 的场景标为 `manual_canary`；复杂 E2E 的未来层级记录在 `execution`，并由 `automation_status` 明确区分 designed、partial 和 implemented。

## 验证

```bash
python3 tools/governance/validate_scenario_test_governance.py
python3 -m unittest tests.test_scenario_test_governance
```

Windows CI 使用 `python` 执行相同命令。

## 变更声明

产品 `feat` / `fix` 的 PR body 必须包含：

```text
Scenario-Test: HLT-003, HLT-004
```

完整规则和复杂 E2E 设计见 `docs/specs/feature-specs/scenario-test-governance.md`。
