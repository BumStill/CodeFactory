### Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| CF-GATE-R1 | 门禁只把「验证类」命令（测试、构建、lint、类型检查、治理校验等）的失败当作必须重跑通过的 blocker；只读探查（cat / ls / grep / find / head 等组合）失败不构成 blocker。判定依据是命令本身做了什么，不靠维护词表 | 测试：两条真实 blocker 命令不再成为 blocker；真实测试失败仍然是 blocker |
| CF-GATE-R2 | 门禁的 blocker 文案用大白话写清楚是哪项检查没过、重跑哪条命令；不出现内部词汇 | 测试 + 内部词汇守卫 |
| CF-GATE-R3 | 失败总结和交付总结里关于改动状态的描述，必须以实际 git 状态为准：已提交未推送 / 已推送 / 仅在工作区未提交 / 已开 PR，四种情况分别如实描述，不得猜测 | 测试：四种状态各一条 |

### Applicable Harnesses
Spec Harness；Compatibility Harness（已有的 gate_events 与 objective）；Observation Harness；AI Collaboration Harness。

### 测试矩阵
- 门禁：探查失败、测试失败、构建失败、治理校验失败，以及探查与测试混在一条命令里的情况。
- 总结：上面四种 git 状态。
