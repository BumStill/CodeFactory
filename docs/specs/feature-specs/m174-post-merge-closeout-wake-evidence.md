# M174 / M174b 验收与交付账本

## 范围与需求映射

接续 `origin/wip/m174` 保存的实现，在当前托管分支合入最新主线。不修改原始 R1–R3，不改治理信任根，不发版。

| 需求 | 可执行验收 | 当前结果 |
| --- | --- | --- |
| R1 等待期间 PR 合并后接续 §8、事实文案与 ended | `m174_merge_during_wait_crosses_process_and_closes_out`：三个独立测试进程依次建立等待、写入 MERGED 观测、投递并消费收尾；真实 SQLite 持久化，断言提示含远端分支清理、最终回答后 ended、重复 drain 为 0 | 通过 |
| R1 接手他人分支，以实际交付结果认领 PR | `m174_cross_process_merged_wait_is_closed_out`：自有与接手分支两个合成会话；工具结果修复交付引用；旧 silent-closeout 与合并后普通等待回答不算已收尾 | 通过 |
| R2 重启结算已合并存量 | 上述子进程调用生产入口 `reconcile_waiting_on_startup`，核对两个 waiting_wake 会话、持久唤醒和 ended | 通过 |
| R2 已关闭 PR 结束 | `m174_closed_pr_final_settles_instead_of_waiting_forever`：关闭 PR 的持久修复唤醒完成并给出最终回答后 ended，原因保持已关闭未合并 | 通过 |
| R3 跨进程和幂等 | 两项跨进程测试使用合成 fixture、真实 SQLite 与生产唤醒投递器；模型回合由断言型 WakeSink 代替，不声称执行了真实模型或真实远端删除 | 通过 |
| 兼容：停止、权限、历史已收尾、限流 | 全部 `thin_derive::tests` 38 项 | 通过 |

## 先失败与最小修复

保存进度原账本记录：合并后仍说“还没合并”和关闭后 WaitingWake != Ended 两项先失败后修复。本次不把该历史记录当作新执行证据。

本次实际观察到失败：同一 PR/分支的交付上限仍为 `pr_only`，`m174_cross_process_merged_wait_is_closed_out` 断言 `left: pr_only / right: ThroughMerge`。随后最小修复交付引用 UPSERT 与观测 UPSERT，使 ceiling 变化也同步；原失败命令重跑通过，全部 38 项派生回归通过。

## 验证命令与结果

- `pnpm install --frozen-lockfile`：通过。
- `pnpm test`：159 files / 1055 tests 通过，包括 lightModeAudit。
- `pnpm build`：通过；仅已有 chunk size 提示。
- `CARGO_INCREMENTAL=0 CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-m174b" cargo test --manifest-path src-tauri/Cargo.toml m174_cross_process_merged_wait_is_closed_out -- --nocapture`：先失败、修复后通过。
- 相同目录执行 `cargo test --manifest-path src-tauri/Cargo.toml thin_derive::tests -- --nocapture`：38 项通过。
- workspace 全量测试首轮仅上述新回归失败；修复后全量重跑结果待回填。
- `python3.11 tools/governance/run_scenario_harness_gate.py --stage local --repo . --policy-repo . --base-ref origin/main --title "fix: settle merged PR sessions through durable closeout (M174)" --body-file docs/specs/feature-specs/m174-post-merge-closeout-wake-evidence.md`：通过。
- `python3.11 tools/governance/validate_repo_governance_baseline.py`：通过。
- 系统 Python 缺 `tomllib`，改用本机 Python 3.11；未改 validator。
- 场景声明移除旧账本未注册的 CAP-007，补上实际受影响 RTE-003；当前场景及 README 校验通过。

## Harness 与 AI Collaboration

- Spec：原始任务规格随保存提交版本化，需求含义无变化。
- Compatibility：历史 SQLite、跨进程持久化、旧收尾记录和交付上限修复。
- Observation：PR 终态事实优先于过期等待投影；保留原有 180 秒观测限频与 GitHub 预算。
- Release：仅授权合并、队列及模板 §8 收尾；本任务不发布。
- AI Collaboration：context scope 为 thin PR 观测、实际工具交付结果、持久唤醒及启动恢复；assumptions 为实际交付结果决定归属，WakeSink 仅替代模型边界；review point 为终态优先级及重复收尾。已发出两项只读独立审查请求，工具返回尚需启动，未把未执行审查计为通过。validation result 为本次先红后绿、38 项状态回归和前端全量测试。

## 真机验收分工与局限

真机验收由编排方发版安装后完成；本任务 **not live、不发版**。安装后分别验证外部 squash/队列合并、接手他人分支合并、重启已合并/关闭存量会话；核对自动 §8、ended、事实文案、幂等及真实分支/工作区清理。测试中的最终回答和外部 PR 观测为合成数据，不冒充真机清理证据。无 UI 布局变更，不新增截图。

Spec Amendment：无。

Scenario-Test: HLT-001, HLT-002, HLT-004, HLT-005, RTE-003
README-Update: reviewed
README-Update-Reason: 内部 PR 会话恢复修复，不改变公开安装、平台或隐私承诺。

Release-Urgency: immediate
