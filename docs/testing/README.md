# 场景测试

本目录的权威机器源是 `scenario-registry.json`。它统一管理业务场景、UI acceptance、运行时 smoke、复杂 E2E 组合、证据等级和执行 gate。

## 当前规模

<!-- scenario-registry-summary:start -->
- 逻辑 Scenario：`27`（P0 `14`，P1 `13`，P2 `0`）
- Complex E2E：`11`（implemented `0`，partially_implemented `10`，designed `1`）
- 剩余自动化缺口：`25`
- PR slice：implemented `9`，partially_implemented `1`，missing `1`
<!-- scenario-registry-summary:end -->

以上区块由 `scenario-registry.json` 派生并由 CI 校验，不手工估算。PR、nightly、release 对同一场景的重复执行只算不同证据层，不重复计数。`implemented 0` 的含义是 11 个复杂旅程目前都还没有同时清零 PR、nightly、真实桌面和 exact release 证据；现有局部测试仍是有效证据，但不能被包装成整条旅程已完成。

E2E-002/E2E-003/E2E-007 已由 `--history-session-smoke` 覆盖真实 SQLite、正式二进制多进程、hard kill 和两次重启的 L2/L4 断言；E2E-007 另由真实浏览器中的 production reducer、MessageList、ToolCallCard 与 MessageInput 覆盖 handback UI。它们接入 required Windows CI、nightly 和 release exact executable。真实桌面 WebView 的完整 provider 路径、旧 schema 矩阵和部分投影失败并发仍是显式缺口，不计为已覆盖。

`gates` 表示当前真实 workflow 覆盖，不表示目标。存在脚本但未接入 CI 的场景标为 `manual_canary`；复杂 E2E 的未来层级记录在 `execution`，并由 `automation_status` 明确区分 designed、partial 和 implemented。

## 验证

```bash
python3 tools/governance/validate_scenario_test_governance.py
python3 tools/governance/scenario_catalog_docs.py
python3 -m unittest tests.test_scenario_test_governance
python3 -m unittest tests.test_scenario_catalog_docs
```

Windows CI 使用 `python` 执行相同命令。

## 变更声明

任何命中产品路径的 PR body 必须包含：

```text
Scenario-Test: HLT-003, HLT-004
```

完整规则和复杂 E2E 设计见 `docs/specs/feature-specs/scenario-test-governance.md`。
