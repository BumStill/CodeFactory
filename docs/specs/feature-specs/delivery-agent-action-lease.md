# M91：交付等待 agent 时不能锁死 agent

## 原始任务（逐字保留）
1. 复现：deliver_changes 因正文不合规返回 agent_action_required 后，同一 objective 的 agent 改好正文再次调用 deliver_changes，应当接着同一次交付往下走，而不是得到 delivery_operation_already_running。先写失败测试。
2. 交付进入 agent_action_required 时，释放 mutation 租约，或者允许同一 objective 的调用方直接接管。
3. 交付处于「等 CI / 等合并队列」时，agent 的再次调用应附着并返回当前进度（PR 号、CI 状态），不能让 agent 误以为卡死、停下来去找编排方。
4. 不得放宽身份校验：不同 objective 仍然互斥。

## Requirements Traceability
| Req ID | 需求 | 最低证据 |
|---|---|---|
| M91-R1 | 同 objective 可接管 agent_action_required 的租约并保留交付身份 | SQLite 集成测试，修正文案后重入、旧 permit 失效 |
| M91-R2 | CI/队列等待附着报告 PR 与当前状态，不误判锁停滞 | 工具输出字段断言 |
| M91-R3 | 不同 objective 不得接管 | 身份冲突集成测试 |

## Applicable Harnesses
Spec、Compatibility、Observation、Release、AI Collaboration Harness。无 UI 布局变更；后台入口仍为 deliver_changes。

## 实现约束与验收
仅合成数据；不改工作流、场景注册表或校验器。保留 objective/worktree/repo 身份门禁与 epoch fencing，活跃 mutation 不可无条件抢占。真实安装版验收由编排方发版后完成：复现无 Scenario-Test 正文，修正后重入；等待 CI 与队列时重入，核对同 PR 与进度；不同 objective 尝试不得获取租约。
