# M179（高）：权限请求「无法绑定到确切 Objective revision」导致 PlatformIncident 连败结束会话

## 证据（v1.91.37/38，已含 M169 #864）
- 10-26 19:11 会话 5dd6b899（M178，trusted，授权交付）：tool 结果 `Permission request could not be bound to the exact Objective revision. No action was executed; the system will reconcile the Objective identity before retrying.`（status=denied），随后 thin=ended「同一个技术失败连续 3 次：agent loop stopped: PlatformIncident」。工作区有 127 个未提交改动，交付中断。
- 同类 10-24 feb07c3a（M166）03:57/04:37 两次停住；M169 修了「追问新建 objective」，本次无追问也发生。
## 需求
- R1 查清 revision 何时变化（同一回合内 objective revision 被谁推进：唤醒、交付工具、计划保存「Plan revision N saved」、安装重启？），写进 PR。
- R2 权限请求绑定以「当前活跃 objective 的最新 revision」重绑定后重试一次，不把内部对账失败当技术失败计入连败；对账无法完成时进入 waiting_wake/waiting_user 并给人话原因，绝不 PlatformIncident 结束会话、绝不丢工作区。
- R3 测试（先失败）：回合中 objective revision 前进后发起权限请求 → 自动重绑定并执行；连续 3 次对账失败 → 不结束会话、有可见原因。
## 约束
- task-template；合成 fixture；授权交付（ceiling: merged）。

Release-Urgency: immediate

## Implementation Notes

- 本任务只实现 R1–R3，不改变权限策略、用户拒绝、精确授权凭据及工作区归属语义。
- Spec / Compatibility / Observation / Release / AI Collaboration Harness：使用合成 SQLite + 权限网关/循环回归、thin 持久唤醒集成测试、全量前端测试与构建、Rust workspace 测试。无布局变化，不扩展 Viewport 范围。
- 真机验收由编排方发版安装后完成：执行长回合并推进 Objective revision，触发权限请求；批准后核对实际工具执行一次；连续三次对账失败后核对可见系统自愈等待、工作区和未提交改动保留。
- 未启动并行角色：任务模板明确禁止 `delegate_tasks`，本次采用单执行流。
