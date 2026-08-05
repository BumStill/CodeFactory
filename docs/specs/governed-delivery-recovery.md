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
| DR-10 | `WAITING_RETRYABLE` 是运行中状态：在同一个 `deliver_changes` 调用内退避、心跳并自动核对，不消耗模型轮次，也不要求用户回复“继续” | tool-loop wait/cancel tests + live CI wait |
| DR-11 | PR/MR 列表查询只有明确 `Absent` 才允许 create；查询错误、非 JSON、字段缺失均为 `Unknown`，必须 fail closed | GitHub/GitLab/gh adapter tests |
| DR-12 | CI 观察不可用必须与真实 check failure 分开；限流、网络、schema 和 required-rules 查询失败不得触发代码修复或证明 CI 绿色 | CI observation classification tests |
| DR-13 | `DeliveryOutcome` 必须满足状态不变量：waiting/recoverable/recovery_class/retry_after/next_action 一致；不合法结果不得渲染成成功或普通阻断 | table-driven contract tests |
| DR-14 | 前端把 `tool_result.status=waiting` 视为活跃远端等待；只有 `Done`/`Error`/取消才结束 streaming | reducer/component tests |

## 状态机

```text
pr_open
  -> gates_pending
       -> waiting_retryable -> 同一工具调用内退避并重新观察
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
- 上述“两轮”只约束需要 agent 修改代码、正文或配置的修复动作；纯远端
  `WAITING_RETRYABLE` 不计入修复次数，也不再通过模型轮询。它保持同一工具 future，
  每 30 秒发脱敏心跳，按结构化 `retry_after_ms` 重新观察，直到真实终态或用户取消。
- README 正文修复、branch update 和 CI rerun 必须幂等。
- 任何 create/merge/release 前置查询均采用三值语义 `Existing / Absent / Unknown`；
  `Unknown` 永远不能降级为 `Absent`。
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
