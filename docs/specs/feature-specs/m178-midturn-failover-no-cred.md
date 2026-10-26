# M178B：回合中途故障切换不得切到无凭据端点

## 原规格（按任务原文保存）

# M178（紧急）：回合进行中的故障切换仍会切到无凭据 OpenRouter 并连败结束会话

## 证据（v1.91.37，含 M164 #853「候选只含有凭据端点」）
会话 aeba516d（M174，chatgpt/gpt-6.1-sol，trusted，授权交付）17:04 后 GPT 五小时窗口达上限、DeepSeek 忙时（14–18）；17:42 thin=ended：
`同一个技术失败连续 3 次：agent loop stopped: agent loop error: 首选端点失败：AUTH_MISSING: openrouter 尚未配置凭据，请在模型设置中保存后重试`
同类此前见 10-22（M152/M155）、10-23 10:40 日志——每次修一条入口，另一条入口仍漏。
## 需求
- R1 全仓搜出**所有**构造路由候选/故障切换的位置（回合开始、回合中途 failover、续跑、唤醒、工具内模型调用、标题/摘要等辅助调用），统一走同一个候选过滤（有凭据 + 窗口内 + 未达上限）。加一个测试/静态检查：任何地方直接构造含无凭据端点的候选都会失败。
- R2 回合进行中所有候选不可用 → 保存进度，进入 waiting_wake（唤醒时间 = 最早可用），不计技术失败、不结束会话；界面一句人话原因（同时修复 waiting_wake 时 reason 为空的问题：17:04 起多个会话 reason 为空）。
- R3 测试（先失败，跨进程）：回合中途 GPT 达上限 + DeepSeek 窗口外 + OpenRouter 无凭据 → waiting_wake + 非空原因 + 到 DeepSeek 窗口自动续跑；不出现 AUTH_MISSING。
## 约束
- task-template；合成 fixture；授权交付（ceiling: merged）。

Release-Urgency: immediate

## Implementation Notes

- 工作在应用已建立的独立 managed worktree 分支上，不另建分支。
- 只恢复保存分支的语义，不引入 rustfmt 噪音；保留主干新修复。
- 以实际设置、订阅账本和当前时间统一评价候选，不依赖上一次准入留下的 skip 缓存。
- 真实安装版验收由编排方在独立发版安装后完成；本任务不发版。

## Requirements Traceability / 测试矩阵

| Req | 最低证据 |
| --- | --- |
| R1 | 路由构造点审计、静态构造门禁、M164 矩阵及统一准入测试 |
| R2 | 回合中途额度变化后最早时段等待、reason 非空、失败计数为零、持久化唤醒 |
| R3 | 子进程写入等待，另一子进程读取并到点消费真实调度，恢复原 root turn，幂等投递 |

## Applicable Harnesses

Spec、Compatibility（既有错误编码与数据库）、Observation（状态及唤醒字段）、Release（PR/CI/合并与紧急 trailer，not live）、AI Collaboration（单执行流；模板禁止 delegate_tasks）。

## AI Collaboration

Context scope：指定任务、保存分支语义、路由/传输/持久状态/调度调用链。
Assumptions：使用 synthetic fixture 和注入时钟，不访问真实凭据，不消耗真实额度。
Review point：防止把数据库重连冒充跨进程，防止用陈旧 skip 缓存丢失最早可用时间。
Validation result：待执行，验收账本和 PR 记录实际结果。
