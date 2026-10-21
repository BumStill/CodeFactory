# M131d 场景覆盖判定一致性（网页抓取登记 + PR/发版同规则）

## 背景

`v1.91.5` 发版批次出现了"同一个提交，PR 绿、发版红"的矛盾结果：

- #793 引入 `src-tauri/src/tools/web_fetch.rs` 与 `src-tauri/src/tools/fixtures/web-fetch-wiki.html`，
  场景注册表里没有任何场景声明这两个文件。
- `pull_request` 阶段：同批次含 `src-tauri/Cargo.toml` / `src-tauri/Cargo.lock` 这类全局产品文件，
  `expand_global_files=True` 把影响面展开为"全部场景"，于是"未映射的产品文件"从未被报出——假绿。
- `release_artifact` 阶段：发版关闭展开（`expand_global_files=False`），同一批次被判成
  "scenario registry coverage gap for product files"，且把已按版本清单豁免的
  `package.json` / `Cargo.toml` / `Cargo.lock` / `tauri.conf.json` 一起列进缺口——假红，且看不出真正的
  未映射文件是哪一个。

结论：覆盖判定不能建立在"展开后的影响集是否为空"上，两个阶段必须使用同一条规则。

## Requirements

| ID | 要求 | 证据 |
| --- | --- | --- |
| R1 | 网页抓取产品面（`web_fetch.rs` 与其 fixture）必须有场景映射，且映射到的自动化真实存在并在 `pull_request` 阶段执行 | `CAP-001`；`tests/test_scenario_registry_web_fetch.py::WebFetchScenarioRegistrationTest` |
| R2 | `pull_request` 与 `release_artifact` 对"未映射产品文件"使用同一判定：全局清单文件由展开语义承载，其余产品文件必须被某场景的 `change_patterns` 显式声明 | `_unmapped_product_files` 被 `validate_impacted_execution` / `validate_change_contract` 共用；`CoverageGapStageParityTest` |
| R3 | 版本清单豁免仍然只在"纯版本同步补丁"生效：四个清单文件里只有版本行变化时可豁免，夹带任何产品文件仍判缺口，且豁免文件不再混入缺口信息 | `VersionManifestExemptionTest` |

## 设计

- `_unmapped_product_files(files, registry)`：排除 `GLOBAL_PRODUCT_FILES` 后，任何不被场景
  `change_patterns` 匹配的产品文件即未映射。全局清单的"影响所有场景"由 `expand_global_files`
  表达，因此它本身不算缺口。
- `validate_impacted_execution(..., fail_on_unmapped=True)`：先判未映射并直接返回具名缺口
  （早于计算展开后的影响集），因此"全局清单把影响面撑满"不再能掩盖缺口。
- `validate_change_contract`：PR body 的 `Scenario-Test:` 声明腿使用同一 helper，缺声明与缺口
  一致地报出。
- `run_scenario_harness_gate.py` 的 `pull_request` 分支显式传
  `expand_global_files=True, fail_on_unmapped=True`，与 `release_artifact` 分支对齐（展开语义不同，
  覆盖判定相同）。
- 注册表新增 `CAP-001`（category `capability_usage`，P1），绑定
  `m131c_tag_prefixes_do_not_consume_unrelated_elements`、
  `literal_local_hosts_are_rejected_without_any_dns_lookup`、
  `ordinary_domains_are_not_ruled_out_by_address_guessing`。

## Primary User Path

用户提交网页抓取相关改动 → PR 阶段 `scenario-gate-pr` 要求 `Scenario-Test:` 命中 `CAP-001` 并跑对应
Rust 测试 → 合并后发版阶段在同一批次上给出与之相同的结论，不再出现"PR 绿、发版红"。

## Applicable Harnesses

- Spec Harness：本文件 + `docs/testing/scenario-registry.json`（机器权威）。
- Release Harness / Compatibility Harness：`release_artifact` 阶段以 `v1.91.4` 为基线实跑通过。
- Payload Harness：`web-fetch-wiki.html` fixture 与 `web_fetch.rs` 同一场景，改动任一侧都要求覆盖。
- AI Collaboration：假设——"全局清单文件不参与缺口判定"属于既有设计意图（`expand_global_files`
  的展开语义），未在本次修改中改变；review point—R2 的两阶段一致性由单元测试锁定。

## 验证

| 命令 | 结果 |
| --- | --- |
| `python3.11 -m unittest tests.test_scenario_registry_web_fetch -v` | 8 passed |
| `python3.11 -m unittest discover -s tools/governance -p 'test_*.py'` | 20 passed |
| `python3.11 tools/governance/run_scenario_harness_gate.py --stage release_artifact --base-ref v1.91.4` | OK（此前假红） |
| `python3.11 tools/governance/run_scenario_harness_gate.py --stage pull_request --base-ref v1.91.4 --body-file <声明 CAP-001 的 body>` | OK |
| 负向控制（`Cargo.lock` + 未登记产品文件） | 两阶段均只报 `not_registered_probe.rs`，不再混入清单文件 |
| `python3.11 tools/governance/validate_repo_governance_baseline.py` | pass |

## 未覆盖

- 未在真实 GitHub runner 上重跑 `scenario-gate-pr`（本地以同一入口 `--stage pull_request` 验证）；
  合并后由必需检查继续把关。
