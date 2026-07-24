# 会话控制收敛与可见恢复

## Requirements Traceability

| Req ID | 要求 | 验证 |
| --- | --- | --- |
| CF-SCC-R1 | `permissions.full_access` 只影响工具权限决策，不得直接选择 `AgentMode::Execute` | Rust dispatch unit + settings compatibility |
| CF-SCC-R2 | 分析、解释、状态查询和诊断请求在 Full access 下仍按普通交互回合处理；除非用户明确要求修改/实施，否则不得因为权限配置扩大成代码交付任务 | Rust command contract + real app |
| CF-SCC-R3 | 明确批准的实施请求和结构化「继续执行」动作仍进入 Execute，不得因 R1 退回重复确认 | Rust dispatch unit + real app |
| CF-SCC-R4 | Interactive/Execute 一次前台回合最多允许 3 次 completion recovery；累计次数不得因 material evidence progress 清零 | Rust failure-first unit |
| CF-SCC-R5 | 连续无进展计数可以在证据进展后清零，但必须与不可重置的累计恢复次数分离 | Rust state-machine unit |
| CF-SCC-R6 | recovery/ready 的内部 prompt 和被拒绝候选回复继续不进入聊天正文；用户必须看到脱敏的恢复状态卡，而不是只看到 `Thinking` | reducer + component + real app |
| CF-SCC-R7 | 恢复状态卡至少显示阶段、恢复次数、继续原因、当前步骤、最近活动时间和累计耗时；不得泄漏内部 prompt 或未脱敏的命令参数 | component + privacy negative assertions |
| CF-SCC-R8 | `tool_call_start`/`tool_result` 在内部恢复期间更新状态卡；失败和等待权限保持可见，完成门禁不能删除整个用户回合的活动证据 | reducer + hydration regression |
| CF-SCC-R9 | 历史加载后应从持久化的 completion state 与 tool call 记录重建简洁恢复摘要；旧数据库无新增字段时保持兼容 | hydration unit + SQLite compatibility |
| CF-SCC-R10 | 前台恢复耗尽后必须发出可见终态和 `Done`；不得继续隐藏执行或把第 4 次拒绝变成新的恢复循环 | Rust event sequence + frontend stream unit |
| CF-SCC-R11 | 取消文案明确表示停止后续生成，不自动回滚已提交、已推送或已执行的外部状态 | component + real app |
| CF-SCC-R12 | PR+CI、真实 CodeFactory App 和精确发布产物验证前保持 `not live` | evidence pack |
| CF-SCC-R13 | 自动事实纠偏只允许进入执行型回合；交付类纠偏还必须核对当前用户明确要求的交付动作。检测不得跨段拼接示例、引用或假设中的关键词，不得把分析/设计回答改写成 `deliver_changes` 等无关执行。内部 `turn_notice` 必须保留可审计的 system 来源，不能冒充新的用户目标 | Rust failure-first regression + exact field-session replay + real app |

## Primary User Paths

### 诊断路径

用户在 Full access 模式下发送「这是怎么了？」。CodeFactory 允许模型读取当前界面、会话、日志、进程与仓库状态，但仍按 Interactive 回合处理；没有明确修改授权时不得把问题自动扩展为代码实现、提交、推送或 PR 修改。

### 执行与恢复路径

用户明确要求实施，或点击结构化「继续执行」后进入 Execute。模型尝试结束但验证证据不足时，正文中的候选草稿和内部 recovery prompt 保持隐藏；同一位置显示一张紧凑状态卡，说明正在补充验证、当前为第几次、最近正在做什么以及为什么还不能结束。累计恢复达到 3 次后，系统交付当前最佳答复与中文验证不完整警告，并关闭流。

### 历史恢复路径

用户切换会话或重启 App 后，进行中的会话不把内部 prompt 当成用户消息，也不重新展示被拒绝草稿；历史活动被折叠为简洁恢复摘要，最终回答与警告保持可读。

### 自纠偏边界

当候选回复声称当前用户要求的交付动作因认证、工具或可检测条件受阻时，系统可以在同一回合发起一次有证据的纠偏。检测必须同时满足：

- 当前回合是 Execute 或 Autonomous 等执行型模式；Interactive 分析回答直接结束，不启动隐藏纠偏循环；
- 当前用户目标本身明确涉及提交、推送、PR、发布或交付；
- 阻塞主张与认证/配置要求出现在同一句或同一局部语境；
- 示例、代码块、引用和假设性方案不作为当前阻塞事实。

若当前目标是分析、设计、解释或诊断，自纠偏不得注入 `deliver_changes`、不得继续无关工具链，也不得让已经完成的正确回答降级为折叠步骤。

## Applicable Harnesses

- Spec Harness：CF-SCC-R1..R13。
- Compatibility Harness：旧 settings、旧 completion state、旧会话 hydration。
- Viewport Harness：1366×768、800×600 下状态卡与输入区不重叠。
- Observation Harness：真实 App 中 Full access 诊断、Execute 恢复和取消路径。
- Payload Harness：状态卡不得泄漏完整命令、凭据、内部 prompt 或大段 tool result。
- AI Collaboration Harness：独立架构/UX 审查、失败测试、最终验证结果。
- Release Harness：PR+CI、安装包与发布 App 精确版本证据。

## 完成边界

只通过 Rust 或 React 单测不算完成。必须证明 Full access 不再改变回合意图、恢复累计上限真实生效、内部恢复期间用户可见进度、历史兼容，以及发布 App 的真实会话行为。
