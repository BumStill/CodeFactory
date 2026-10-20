# M128：交付前本地跑治理基线校验（同族第 3 次：规格措辞触发 OBJECTIVE_MANUAL_TECHNICAL_HANDOFF）

## 证据
- #751（M119 规格 :14）、#769（m127 规格 :14）以及 M119 首版：governance-baseline 失败 `OBJECTIVE_MANUAL_TECHNICAL_HANDOFF ... system-owned technical state requires manual '重试'`。每次都要等 CI 失败→会话发现→改→再跑 CI（20–40 分钟/次）；#769 还因会话没发现而卡数小时。
## 需求
- R1 thin `deliver_changes` 在推送前对本次改动运行 `python tools/governance/validate_repo_governance_baseline.py`（以及 check-frontend 中可本地运行的 PR 正文/README 校验，若 P1 #771 已覆盖则复用），失败时**不推送**，工具结果直接返回失败规则、文件:行号与规则说明，让模型修。
- R2 校验耗时需可控（给出实测耗时）；校验脚本本身不可用时降级为警告并在 PR 正文注明，不阻断。
### 原始需求证据（R3 原文）
- R3 在 task-template / 系统提示中给模型一句话说明该规则的含义与正确写法（系统技术状态写成「系统自动重试/自愈」，人工动作只能是产品层面的用户决策）。
### 测试需求
- R4 测试（先失败）：改动包含触发该规则的规格 → deliver 返回失败且未推送；修正后 → 推送成功。
## 约束
- 合成 fixture。授权交付（ceiling: merged）。PR 正文带 `Release-Urgency: immediate`。

## Implementation Notes

- R1 复用 P1 `prepare_thin_pr_body` 的 README / scenario 双校验；额外调用既有 baseline 与 `validate_objective_non_interruption_contract.py --base origin/<default>`。后者是 CI 单独执行的规则，baseline 本身并不包含它。
- R2 每个治理脚本最多 10 秒，总预算 20 秒；外层仍使用既有 30 秒交付命令边界。结构化规则失败阻断；脚本缺失、启动失败、崩溃、超时或输出协议不可用降级，警告写入正文及工具 steps；不降级已知规则失败。
- R3 在公共系统提示中给出一句规范，不改变用户决策边界。不修改仓库外任务模板。
- 不改 trust roots；托管分支继续开发；本次仅合并、不执行发版。

## Requirements Traceability / 测试矩阵

| Req ID | 最小证据 |
| --- | --- |
| R1/R4 | 真实既有校验器 + synthetic Git fixture：违规规格阻断且无 push，修正后通过；thin forge 验证失败时零 push/PR，成功后 push |
| R2 | synthetic 缺失、崩溃、超时、合法 baseline 失败；警告写入正文；实际仓库计时 |
| R3 | 公共系统提示规范句断言 |

## Applicable Harnesses / AI Collaboration

- Spec Harness：原始需求逐字保存；上述 Req 映射。
- Compatibility Harness：非 CodeFactory 仓库无需治理脚本；P1 正文契约保持原入口。
- Observation Harness：失败保留校验器规则/路径/行号；成功及降级记录耗时与警告。
- Release Harness：仅 PR/CI/队列交接；真实安装版验收由编排方单独发版后执行（not live）。
- AI Collaboration Harness：context scope 为 thin 推送前校验及公共提示；assumptions 为脚本退出码与既有 JSON/诊断协议；review point 为不把可识别规则失败误作不可用；validation 为回归测试、真实脚本与 CI。
- 按任务模板不调用 delegate_tasks；无 UI/viewport 变更。
