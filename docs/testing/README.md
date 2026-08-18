# 场景测试

本目录的权威机器源是 `scenario-registry.json`。它统一管理业务场景、UI acceptance、运行时 smoke、复杂 E2E 组合、证据等级和执行 gate。

## 当前规模

- 18 个去重后的逻辑 Scenario；
- 6 个复杂真实 E2E 组合设计；
- 其中 4 个 `partially_implemented`、2 个 `designed`；
- PR、nightly、release 对同一场景的重复执行只算不同证据层，不重复计数。

E2E-002/E2E-003 已由 `--history-session-smoke` 覆盖真实 SQLite、正式二进制多进程、hard kill 和两次重启的 L2/L4 断言，并接入 required Windows CI、nightly 和 release exact executable。真实桌面 WebView 的 L3 操作、旧 schema 矩阵和部分投影失败并发仍是显式缺口，不计为已覆盖。

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
