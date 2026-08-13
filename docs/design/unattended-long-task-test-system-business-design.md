# 长任务无人参与测试系统：业务设计

## 问题与目标

CodeFactory 的核心承诺不是“能重试”，而是用户授权一个长任务后，在 provider 波动、工具失败、CI 等待、应用退出和电脑重启之间仍由系统负责推进，直到完成、显式取消，或遇到真正缺少核心输入的边界。

现有测试大量证明单个 reducer、状态转换或 adapter，但没有持续拦截以下真实回归：系统不再回复而必须由用户说“继续”、同一恢复动作无限重领、进程重启后重复副作用、以及本地测试通过却提前宣称交付完成。

本设计把这些行为变成 required checks，并满足以下业务结果：

- 一次任务只需要一次初始授权；可恢复故障不得追加人类操作。
- 同一任务保持 `session_id`、`root_turn_id`、`objective_id`、checkpoint、receipt 和 delivery identity。
- 所有外部 mutation 至多一次；不确定时先观察/对账，不能盲目重放。
- 技术恢复有可证明的上限，不能通过进程重启、同一行重领或换 route 重置预算。
- 完成状态由用户约定的交付边界决定，而不是由一次模型回复、一次本地测试或一次 CI 绿灯决定。

## 需求

| ID | 需求 | P0 验收 |
| --- | --- | --- |
| ULT-R1 | 一次授权 | `user_message_count=1` 且 `human_prompt_count=0` |
| ULT-R2 | 跨进程连续 | 真正终止子进程，再以相同 SQLite 恢复相同 objective |
| ULT-R3 | 副作用幂等 | mutation receipt 唯一，工作区结果正确 |
| ULT-R4 | 恢复收敛 | 同签名 5 次、同 objective 20 次按实际 claim 计数并终止循环 |
| ULT-R5 | 增量指令 | 目标、补充约束、纠错、“继续/可以”按顺序应用到同一开放任务 |
| ULT-R6 | 持久停止 | 会话全部 live objectives 取消后跨重启不复活 |
| ULT-R7 | 真实完成边界 | local、PR、CI、merge、release、artifact 分阶段记录，不提前 completed |
| ULT-R8 | 兼容历史数据 | fresh DB 和版本化旧 schema fixture 均走正式 migrations |
| ULT-R9 | 无抖动门禁 | fault seed 固定；失败不靠自动 rerun 洗绿 |
| ULT-R10 | 隐私 | 历史数据只用于归纳形状，仓库仅保存匿名合成场景 |

## 历史场景的使用边界

本地历史 session 是“场景发现源”，不是测试 fixture。抽取程序只允许输出聚合计数、typed state 轨迹、故障类别和持续时间区间；禁止输出会话 ID、对话原文、本地路径、账号或凭据。

当前场景目录位于 `docs/testing/history-derived-long-task-scenarios.json`。真实观察到的主要形状包括：长期会话中反复出现简短“继续/可以”，以及同一 remediation 行多次 claim 而未收敛。目录中的 prompt、文件名和 provider 响应全部为合成数据。

## 发布判定

- PR：跨进程 hermetic smoke、恢复预算、续接/取消、数据库迁移必须通过。
- Nightly：扩大 provider/tool/permission/browser/delivery fault matrix，并执行增量 steer 场景。
- Release：在正式 Windows executable 上重复 P0 跨进程 smoke；安装后的人工验收只补 UI/OS 边界，不能替代自动断言。
- Production：只读汇总 ownerless、重复副作用、technical CTA、恢复耗时和虚假完成指标；任一零容忍指标非零即开 incident。

## 非目标

- 不把真实用户对话或数据库提交到仓库。
- 不要求 CI 调真实付费 provider 或真实 GitHub 仓库。
- 不用 UI 截图代替 SQLite、receipt 和 objective 状态断言。
- 不把所有组合塞进 PR 门禁；PR 保持少而强，广泛组合进入 nightly/release。
