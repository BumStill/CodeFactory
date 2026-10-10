# CF-GATE 自验收与 PR 正文

## 变更
- CF-GATE-R1：按 shell 语句/管道的实际执行入口识别验证，不再因为文件名或搜索内容含测试名而误判。混合命令保留真实检查。
- CF-GATE-R2：失败检查提示说明没有通过的检查及修复后重跑的原命令，结构化证据字段保持兼容。
- CF-GATE-R3：读取 git 基线后提交数及未出现在远端引用中的提交数，两类总结共用状态描述；观测失败不猜测已提交或未推送。

## 自验收表 / Req ↔ 测试
| 验收项 | 结果 | 证据 |
| --- | --- | --- |
| R1 两条真实探查、含测试名文件和搜索参数 | PASS | cf_gate_exploration_never_opens_a_failed_check：先失败后通过 |
| R1 测试、构建、治理检查失败仍阻止完成 | PASS | cf_gate_real_checks_still_block_with_plain_instructions |
| R1 探查 + 测试混合 | PASS | 同上 cat package.json && pnpm test |
| R2 明确重跑原命令、内部词汇守卫 | PASS | cf_gate_real_checks_still_block_with_plain_instructions：先失败后通过 |
| R3 未观测 git 不声称已提交 | PASS | cf_gate_unobserved_git_state_never_claims_a_commit：先失败后通过 |
| R3 工作区未提交 / 本地提交未推送 / 已推送 / 已开 PR | PASS | cf_gate_four_git_states_are_honest_in_both_reports：两类总结均断言 |
| Rust 全工作区 | PASS | 专用目录 cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast |
| 前端全量、lightModeAudit | PASS | pnpm test：134 文件、873 测试 |
| 类型检查 / 构建 | PASS | pnpm build；仅大 chunk 警告 |
| diff / 治理基线 | PASS | git diff --check；validate_repo_governance_baseline.py |

## Harness / AI Collaboration
Spec Harness：用户规格原样落盘，无 Spec Amendment。
Compatibility Harness：agent-core 150 测试、总结 14 测试和 Rust 全工作区通过；gate_events / objective 持久化字段不变。
Observation Harness：检查及原命令保留在结构化证据；git 观测失败明确未知。
AI Collaboration Harness：context scope 为任务文件、模板、repo profile、门禁及总结代码；assumptions 为 git 远端引用反映本机已知推送状态，不伪称实时网络状态；review point 为全量回归和未知状态负控；validation result 如表。用户要求单执行流，未调用 delegate_tasks。
无 frontend / 布局改动，浏览器验收和截图不适用。后端自动路径不添加仅 GUI 入口。

Scenario-Test: HLT-001, HLT-002, HLT-005, CXD-001, CXD-002, RTE-003
README-Update: not-needed
README-Update-Reason: 内部门禁及总结修复，不改变公开产品、安装、平台或隐私承诺。

## 交付边界
授权 ceiling: pr_only；开 PR 后停止，不等待 CI、不合并、不发版。not live；正式机验收在后续合并/发布任务完成。模板 §8 合并后收尾暂不适用，不删除受管工作区。专用构建目录为 .codefactory-cache/cargo-target-cf-gate。
