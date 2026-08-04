# 受控交付失败恢复规格

## 背景与目标

CodeFactory 的受控交付必须把一次已授权任务持续推进到用户选择的 ceiling。CI 失败、
PR 正文缺少治理字段、分支落后或远端暂时失败都不是新的用户意图，也不能被统一压成
“阻断后总结”。只有无法安全推断的代码冲突、授权撤销、PR head 漂移和高风险歧义才
需要人工介入。

## Requirements Traceability

| Req ID | 要求 | 权威证据 |
| --- | --- | --- |
| DR-1 | 只有用户当前消息中的显式只读约束才暂停既有交付授权；诊断、追问和纠错不固定回合能力 | `dispatch` intent tests |
| DR-2 | `deliver_changes` 创建 PR 前必须生成唯一、非占位的 README decision/reason | delivery unit tests + real PR body |
| DR-3 | 复用已有 PR 时必须保留正文其余内容，并自动收敛缺失、重复、非法或占位的 README 字段 | delivery adapter tests + real PR body |
| DR-4 | README required check 必须读取当前 PR 正文；正文编辑会触发新 run，rerun 也不能困在旧 event snapshot | workflow contract test + GitHub Actions run |
| DR-5 | 可恢复的 blocked tool result 必须回到执行循环，执行 `next_action` 后续跑；同一回合最多自动恢复两次 | agent-loop policy/run tests |
| DR-6 | `BEHIND` 必须自动 update branch、同步本地 head 并重跑 required checks，不能被描述成被动等待 | delivery state-machine tests + real PR merge state |
| DR-7 | cancelled、timed_out、stale、startup_failure 等基础设施结论允许有界 rerun；普通测试 failure 不盲重跑，必须把失败 check 和最小恢复动作交回实现循环 | CI classification/recovery tests |
| DR-8 | 任何恢复都不得使用 `--admin`、force push 或 ruleset bypass；合并仍绑定 expected head | merge argv tests + live ruleset |
| DR-9 | 完成边界是 required checks 全绿、PR 合并、批次发布成功、安装产物与 `latest.json` 可验证 | remote PR/release/artifact evidence |

## 状态机

```text
pr_open
  -> gates_pending
  -> failure_classified
       -> metadata_repaired -> gates_pending
       -> branch_updated -> gates_pending
       -> infrastructure_rerun -> gates_pending
       -> implementation_required -> agent 修复/commit/push -> gates_pending
       -> human_required -> blocked
  -> ci_green
  -> merge_queued
       -> branch_updated -> gates_pending
       -> merged
  -> release_triggered
  -> live_verified
```

## 有界恢复规则

- 同一根任务内可恢复的交付结果最多自动续跑两轮；每轮必须消费明确的
  `next_action`，不能原样重复同一外部写动作。
- README 正文修复、branch update 和 CI rerun 必须幂等。
- PR head 与回执绑定 SHA 不一致时旧授权失效，禁止自动合并。
- 普通测试失败不允许无条件 rerun；先返回失败 check、结论和可执行诊断入口，由
  agent 修改代码或测试后再 push。
- 恢复用尽后输出具体失败签名、已尝试动作和下一步，不输出泛化的权限或“本回合”文案。

## Primary User Path

用户授权上线 → agent 本地验证 → `deliver_changes` 创建或复用 PR → 自动补齐 README
审计字段 → required checks → 若失败则分类和有界恢复 → 自动合并 → 按 release cadence
触发批次发版 → 验证公开安装包、签名/校验文件和 `latest.json`。

## 回滚边界

本变更不修改数据库 schema。若恢复编排产生异常，可回滚本 PR；GitHub ruleset、required
checks 和 release workflow 保持原有保护，不需要降低门禁或回滚生产数据。
