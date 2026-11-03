# M223：GitHub 侧瞬时故障不得让整次发版失败

## 1. 现象与证据

2026-11-03 晚到 11-04 凌晨，同一夜的自动发版连续三次被人为介入，原因都不是产品代码：

| 现场 | 观测到的原始状态 | 旧行为 |
|---|---|---|
| auto-release run 41642171366 | 「Wait for guarded version bump merge」轮询 `gh api graphql` 时收到 `HTTP 502: Bad Gateway` | `set -e` 让整次发版失败 |
| release.yml run 41669986868 | `build-macos` 被取消，其余作业成功 | 留下一个 Draft release 等人工处理 |
| auto-release run 41689888439 | 版本 PR 必需检查 `remote-real-app-gui` 被判 fail；job 注解原文为「The job was not acquired by Runner of type hosted even after multiple attempts」 | 视为真实测试失败，整次发版失败 |

共同结构：**观测失败被当成了结论**。在「等版本 PR 合入」这一步，任何非 `SUCCESS` 的结论、任何读不到 PR 的瞬间，都被写成了一个关于世界的判断（"测试挂了"），于是 `exit 1`。三次都只需重跑或重切，但每次都要编排方介入，`immediate` 发版被拖慢几十分钟。

## 2. 归因与修复决策

| # | 需求（源自任务 R1–R5） | 决策 |
|---|---|---|
| R1 | 轮询 GitHub API 遇 5xx / 网络错误按有限次数退避重试 | 等待步骤里先有 `gh_retry()`（5 次，间隔 10/20/30/40s，每次记录第几次和失败命令），`observe_pr()` 用同一节奏读版本 PR。**读不到不是结论**：连续读不到只打印 `::warning::` 并继续观察，直到 120 分钟预算耗尽才报错，且报错文案说明是观测失败。 |
| R2 | 必需检查 CANCELLED，或注解含 `not acquired by Runner` / capacity 一类基础设施原因 → 判定为基础设施并重跑 | 判定逻辑集中到 `tools/release/classify_ci_infra.py`。等待步骤把 rollup（含 `startedAt`、check-run `id`）、`gh run view --json jobs` 的作业/步骤、以及每个红项的 check-run 注解一起交给它；注释与日志走同一套 `infra`/`real` 词表。有真实断言证据时永远优先判 real。 |
| R3 | release.yml 作业因排不到 runner 被取消时自动重跑失败作业（最多 2 次），不得留下 Draft | 新增 `release-recover.yml`：由 `workflow_run`（Release / completed）触发，从默认分支执行，只对「所有红掉的作业都是基础设施原因」的运行调用 `gh run rerun --failed`；`run_attempt >= 3` 后不再自动重跑，并把「Draft 会被下一次发版复用」写进日志。真实失败不重跑，Release 本身就是信号。 |
| R4 | 重试要写进日志（第几次、原因），最终失败要说明是基础设施还是测试 | 每条日志都带 `(retry n/2)`、`attempt n/5`、`settling after an infrastructure rerun (k grace round(s) left)`；最终失败文案区分 `failed with a real result` / `no recognisable infrastructure signal` / `persisted after N automatic retries` / `no rerunnable workflow run id`。 |
| R5 | 判定逻辑可测，用本次三种原始输出做用例 | 判定全部收进一个纯函数脚本，`tests/test_ci_infra_retry.py` 覆盖决策表（含三次事故的原始形状），`tests/test_auto_release_wait_loop.py` 则把 workflow 里**真实的 shell 步骤**抽出来在 `bash -euo pipefail` 下用假 `gh` 跑，断言"重跑后继续发版 / 真实失败不重跑 / 读不到不是结论 / 重试有上限"。 |

### 关键实现细节

- **最新一次尝试优先**：重跑会在同一个 head SHA 上再挂一个 check-run，rollup 里会同时出现旧的 `FAILURE` 和新的一次。判定按 `startedAt` 取最新一条，否则"救了它的重跑"会被"它想救的旧失败"继续拖死。
- **失败关闭的默认值**：只有被**正面识别**为基础设施的失败才可重跑。未知步骤名、没有任何证据、混合信号一律判 real 并停止。宁可让人看一眼，也不把真实回归重跑掉。
- **重跑后的宽限期**：发出重跑请求后连续 3 轮只观察不判定（旧结论还在 rollup 上），避免用过期证据烧掉重试预算。
- **预算**：自动重跑最多 2 次；等待窗口从 60 分钟延长到 120 分钟，使"一次重跑 + 重跑后的完整 CI"能落在同一窗口内。

## 3. 需求追溯

| Req ID | 实现 | 验证 |
|---|---|---|
| R1 | `gh_retry()` / `observe_pr()`（`.github/workflows/auto-release.yml`） | `test_unreadable_pr_is_not_a_verdict`（两次 502 后成功，发版继续并报出 `could not read version PR`） |
| R2 | `tools/release/classify_ci_infra.py`（`classify_step` / `classify_log` / `collect_failed_steps` / `latest_attempts`） | `test_runner_capacity_annotation_is_retried`、`test_runner_setup_failure_is_retried`、`test_newest_attempt_wins_over_a_superseded_failure` |
| R3 | `.github/workflows/release-recover.yml` | `ReleaseRecoveryTests`（cancelled build → 重跑；真实 build 失败 → 不重跑；第 3 次尝试 → 停止） |
| R4 | 三处 `::warning::` / `::error::` 文案 | `test_automatic_retries_are_bounded` 断言 `persisted after 2 automatic retries` |
| R5 | 两个测试模块 | `python3 -m unittest tests.test_ci_infra_retry tests.test_auto_release_wait_loop`（34 例） |

## 4. 适用 Harness 与证据等级

- **Spec Harness**：本文件 + Requirements Traceability。
- **Release Harness / Observation Harness**：改动落在发布链路；`python tools/governance/validate_ci_workflows.py`、`validate_python_utf8_contract.py`、`validate_repo_governance_baseline.py`、`run_scenario_harness_gate.py --stage local` 全部通过。
- **Scenario 覆盖**：`.github/workflows/auto-release.yml` 命中 `HLT-001, HLT-004, UI-005, E2E-006`（PR 声明）。
- 证据等级：本地 shell 级功能验证为 **boundary**；本改动真正的验证是"下一次真实发版不再需要人工 rerun"，由 `release-recover.yml` 的日志与 Auto Release 的 `::warning::` 记录承载。

## 2026-11-04 审查更正（以原任务 R1–R5 为准）

首版实现与文档中的「PR 重跑最多两次、等待 120 分钟」不符合原任务，已撤回：
- R2：版本 PR **每个检查最多三次**基础设施重跑；计数按检查名保存。步骤 `timeout-minutes: 60` 覆盖 API 退避、轮询与重跑等待的全部时间，并保留显式 deadline。
- R3：Release 仍是最多两次重跑（总共三次 attempt），耗尽后以非零结果明确失败，不能把未发布的 Draft 声称为成功。
- R2 注解使用 detailsUrl 中的数值 job 标识读取，不能拿 GraphQL node ID 调 REST check-runs 接口。
- 合并要求 classifier 判断六个必需检查全部为 SUCCESS，额外的无关绿检查不能补足数量。

CI 修复记录：Windows 第一次跨进程 smoke 在 phase-two 的硬编码 45s 超时，重跑执行通过；但旧尝试的异常收据仍被可信聚合器拒绝。未修改或削弱 validator；同步 main 并推送新 head 后重新从干净运行验证。三处 restart wait 统一采用已有的可配置 worker_observation_budget。此前 `cargo test --lib unattended_smoke::tests` 执行零个测试，不算验证证据；正式二进制跨进程 smoke 与 agent-core 单测才是有效证据。

原任务 Release-Urgency: default；不擅自触发发版。下一次真实发版与安装版验收由编排方另行发起，检查自动重试日志、最终非 Draft、完整平台资产及更新元数据。

## 5. 已知边界

- `release-recover.yml` 不能把已经发出的 Draft 撤回：它选择让重跑把它做完，而不是删除已经上传的资产。超过重试预算时保留 Draft 并在日志里写明"下一次发版会复用"。
- 判定词表是**正面清单**：GitHub 若出现新的基础设施失败措辞，会被判成 real 并停下来等人——这是刻意的方向（宁可多一次人工确认，也不掩盖真实回归）。扩充词表时请同时补 `tests/test_ci_infra_retry.py` 的用例。
