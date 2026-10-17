# CI 检查 workflow 语法，已知偶发测试自动重跑一次（D + E）

## 背景与用户决定
- **D**：#674 合入了一个写坏的 YAML（`if:` 未加引号，值里含 `: `），合并队列整整 1 小时 20 分钟没有运行；而 CI 根本不检查 workflow 文件本身。
- **E**：同一批偶发测试反复拦住合并，每次都要人工重跑，甚至推新提交从头跑，每次多花约 25 分钟。例子：Windows 无头验收截图失败（M64）、`admission_records_a_wording_constraint…`（M93，已由 #666 修好）。
- 用户 2026-10-17 授权：可修改 CI workflow（`.github/workflows/*.yml`）与必要的场景注册表条目；候选 PR 不能自证，`scenario-gate-pr` 因 cannot self-modify 失败属预期，由编排方处理，不得为让它变绿而绕过门禁；其它检查必须全绿。

## 决定
1. workflow 语法校验放进**已有**的必需检查 `governance-baseline` job，不新增必需检查名字，不改规则集。
2. 使用固定版本 actionlint（下载时校验 sha256）校验 `.github/workflows/*.yml`，再用 YAML 解析覆盖 `.github/rulesets/*.json` 与 `.github/` 下其它 yml。
3. 引入独立的「已知偶发」名单 `docs/testing/flaky-quarantine.json`；只有名单内、且输出确实命中的测试失败，才在**同一个 job 内**自动重跑一次；重跑通过算通过并在 job summary 标注；不在名单上的失败照常失败。
4. 名单不能跳过测试：runner 始终至少执行一次命令；过期或字段缺失的条目由校验器报错（fail closed）。

## Requirements Traceability
| Req ID | 要求 | 最低证据 |
| --- | --- | --- |
| D1 | `governance-baseline` job 在合并前用固定版本 actionlint 校验 `.github/workflows/*.yml`，下载校验 sha256 | `tools/governance/validate_ci_workflows.py --actionlint <bin>` 在坏 workflow 上退出非 0、在本仓库 workflow 上退出 0 |
| D2 | 用 YAML 解析校验 `.github/workflows/*.yml` 与 `.github/` 下其它 yml；用 JSON 解析校验 `.github/rulesets/*.json` | `test_ci_workflow_lint.py` 中坏 YAML / 坏 JSON 用例失败、好文件用例通过 |
| D3 | 用合成坏 workflow（未加引号且含 `: ` 的 `if`）断言被拦下，好 workflow 通过 | `test_bad_unquoted_colon_in_if_is_rejected`、`test_good_workflow_passes` |
| E1 | 已知偶发名单为独立文件，每项写明测试名、来源证据、到期日期 | `docs/testing/flaky-quarantine.json` + 校验器 `--check-only` |
| E2 | 名单内测试失败时在同一 job 内自动重跑一次，重跑通过算通过，并在 job summary 标注「偶发，已重跑」 | `test_quarantined_failure_passes_after_retry`（counter 证明执行两次 + summary 断言） |
| E3 | 不在名单上的失败照常失败；名单不得被用来跳过测试 | `test_off_list_failure_fails`、`test_passing_test_is_not_retried`（只执行一次） |
| E4 | 过期或字段缺失的条目由校验器报错（fail closed），且不执行被测命令 | `test_expired_entry_fails_closed`、`test_missing_field_is_rejected` |
| E5 | PR 附一次真实运行证据：坏 workflow 被拦下的运行 + 名单内测试重跑成功的运行 | PR 正文中的本地/CI 运行日志摘要 |

## Applicable Harnesses
- Spec Harness：本规格 + Req → 测试映射。
- Release Harness：改动只落在 CI 与校验脚本，验证方式为本地运行同一入口；`governance-baseline` 的 CI 运行本身即是 release-facing 证据。
- AI Collaboration Harness：context scope（CI workflow、治理校验脚本、场景注册表）、assumptions（actionlint 可离线下载二进制；Windows 上 `pnpm` 需经 `cmd /c`）、review point（门禁是否 fail closed、名单是否可能被当作跳过手段）、validation result（见 PR）。
- 不触发 Viewport / Payload / Compatibility Harness：无 UI、无文件输入协议、无 provider 路由变更。

## 测试矩阵
| 用例 | 场景 | 期望 |
| --- | --- | --- |
| D-good | 合法 workflow + 合法 ruleset | 校验器退出 0 |
| D-bad-if | `if: ${{ ... 'merge-queue: armed' }}` 未加引号 | 退出非 0，输出指明文件 |
| D-bad-json | 截断的 ruleset JSON | 退出非 0，输出指明文件 |
| D-bad-yml | `.github/` 下其它坏 yml | 退出非 0 |
| D-actionlint-fail | 假 actionlint 退出 1 | 门禁退出非 0，输出含 actionlint |
| E-off-list | 失败测试不在名单 | 退出非 0，只执行一次 |
| E-retry-pass | 名单内测试首次失败、重跑通过 | 退出 0，执行两次，summary 标注偶发已重跑 |
| E-retry-fail | 名单内测试两次都失败 | 退出非 0 |
| E-empty-list | 命令一次通过 | 退出 0，不重跑 |
| E-expired | 条目已过期 | 退出非 0，不执行命令 |
| E-missing-field | 条目缺 evidence | 退出非 0 |
| E-job-scope | 条目属于别的 job | 退出非 0，不重跑 |

## Implementation Notes
- actionlint 版本与 sha256 固定在 workflow 的环境变量里，脚本只接受显式 `--actionlint <path>`，不在运行时自行下载，避免校验器引入网络依赖。
- 重跑 runner 通过 `-- <command...>` 接收原命令，Windows 上对 `.cmd`/`.bat`（如 `pnpm`）自动改用 `cmd /c`，保证与原来的 shell 行为一致。
