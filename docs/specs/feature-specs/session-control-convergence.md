# 会话控制收敛与可见恢复

## Requirements Traceability

| Req ID | 要求 | 验证 |
| --- | --- | --- |
| CF-SCC-R1 | `permissions.full_access` 只影响工具权限决策，不得直接选择 `AgentMode::Execute` | Rust dispatch unit + settings compatibility |
| CF-SCC-R2 | 分析、解释、状态查询和诊断请求在 Full access 下仍按普通交互回合处理；除非用户明确要求修改/实施，否则不得因为权限配置扩大成代码交付任务 | Rust command contract + real app |
| CF-SCC-R3 | 明确批准的实施请求和结构化「继续执行」动作仍进入 Execute，不得因 R1 退回重复确认 | Rust dispatch unit + real app |
| CF-SCC-R4 | Interactive/Execute 单个 root turn 最多允许 1 次定向 completion recovery；恢复提示只列尚缺 evidence，累计次数不得因普通读取或跨 segment 清零。第二次仍不足必须形成唯一的 `verification_incomplete` 终态，不得再次生成候选回复 | Rust failure-first unit + continuity integration |
| CF-SCC-R5 | 连续无进展计数可以在证据进展后清零，但必须与不可重置的累计恢复次数分离 | Rust state-machine unit |
| CF-SCC-R6 | recovery/ready 的内部 prompt 和被拒绝候选回复继续不进入聊天正文；用户必须看到脱敏的恢复状态卡，而不是只看到 `Thinking` | reducer + component + real app |
| CF-SCC-R7 | 恢复状态卡至少显示阶段、恢复次数、继续原因、当前步骤、最近活动时间和累计耗时；不得泄漏内部 prompt 或未脱敏的命令参数 | component + privacy negative assertions |
| CF-SCC-R8 | `tool_call_start`/`tool_result` 在内部恢复期间更新状态卡；失败和等待权限保持可见，完成门禁不能删除整个用户回合的活动证据 | reducer + hydration regression |
| CF-SCC-R9 | 历史加载后应从持久化的 completion state 与 tool call 记录重建简洁恢复摘要；旧数据库无新增字段时保持兼容 | hydration unit + SQLite compatibility |
| CF-SCC-R10 | 当前 segment 恢复耗尽后必须 checkpoint 并自动续段，或形成带具体 blocker 的可恢复终态；未完成时不得发成功/空 `Done`，也不得把第 4 次拒绝变成隐藏循环 | Rust event sequence + continuity integration + frontend stream unit |
| CF-SCC-R11 | 取消文案明确表示停止后续生成，不自动回滚已提交、已推送或已执行的外部状态 | component + real app |
| CF-SCC-R12 | PR+CI、真实 CodeFactory App 和精确发布产物验证前保持 `not live` | evidence pack |
| CF-SCC-R13 | 自动事实纠偏只允许进入执行型回合；交付类纠偏还必须核对当前用户明确要求的交付动作。检测不得跨段拼接示例、引用或假设中的关键词，不得把分析/设计回答改写成 `deliver_changes` 等无关执行。内部 `turn_notice` 必须保留可审计的 system 来源，不能冒充新的用户目标 | Rust failure-first regression + exact field-session replay + real app |
| CF-SCC-R14 | 同一回合中，相同工作目录、相同命令的确定性本地验证在 workspace 未发生新 mutation 时只执行一次；后续相同调用复用已有成功结果。失败验证、远端状态观察、Runtime/Functional Probe 不得复用；任何 workspace mutation 都保守失效已有本地验证结果 | Rust failure-first loop regression |
| CF-SCC-R15 | completion evidence 满足后，产品 Autonomous 的最后一轮只生成用户总结，不再执行工具、不触发事实纠偏，也不把重复绿测记作新进展。即使模型在该轮返回 tool call，也必须保持未执行并最多重试一次纯总结；Benchmark 保留原 coverage audit 语义 | Rust event/tool-execution sequence + real app |
| CF-SCC-R16 | 每个用户 root turn 必须在模型调用前形成独立于权限设置的 `review_only / implement / deliver` capability。Full access 只能放宽已允许工具的审批，不能扩大 capability | Rust dispatch + settings compatibility |
| CF-SCC-R17 | `review_only` 只能看到并执行读取、搜索、状态探测和无副作用验证；`write/edit`、变更型 shell、交付、并行/委派以及未知 MCP 工具必须在 AgentLoop 结构层拒绝，不能依赖模型自律 | scripted transport + backend call counter + temp repo |
| CF-SCC-R18 | `implement` 允许本地实现和验证但不允许 commit/push/PR/merge/release/deploy；只有当前用户明确要求交付，或明确批准包含交付的上一方案时，才进入 `deliver` | dispatch inheritance + bash/tool policy + real app |
| CF-SCC-R19 | 结构门禁拒绝必须落为 `denied` 工具 outcome 并继续形成一次正常用户答复；内部拒绝原因不能冒充新的用户目标或触发隐藏交付纠偏 | event sequence + persistence + hydration |

## Primary User Paths

### 诊断路径

用户在 Full access 模式下发送「这是怎么了？」。CodeFactory 允许模型读取当前界面、会话、日志、进程与仓库状态，但仍按 Interactive 回合处理；没有明确修改授权时不得把问题自动扩展为代码实现、提交、推送或 PR 修改。

### 执行与恢复路径

用户明确要求实施，或点击结构化「继续执行」后进入 Execute。仅实施请求得到 `implement`，包含提交、PR、合并、发布或上线的明确请求得到 `deliver`。模型尝试结束但验证证据不足时，正文中的候选草稿和内部 recovery prompt 保持隐藏；同一位置显示一句紧凑状态，说明正在补充哪一项证据。每个 root turn 只允许一次定向恢复；仍不足时形成唯一的 `verification_incomplete` 结果，不能继续生成候选—拒绝—重试循环。

### 历史恢复路径

用户切换会话或重启 App 后，进行中的会话不把内部 prompt 当成用户消息，也不重新展示被拒绝草稿；历史活动被折叠为简洁恢复摘要，最终回答与警告保持可读。

### 自纠偏边界

当候选回复声称当前用户要求的交付动作因认证、工具或可检测条件受阻时，系统可以在同一回合发起一次有证据的纠偏。检测必须同时满足：

- 当前回合是 Execute 或 Autonomous 等执行型模式；Interactive 分析回答直接结束，不启动隐藏纠偏循环；
- 当前用户目标本身明确涉及提交、推送、PR、发布或交付；若本轮只是“做吧/继续”等批准短语，必须继承上一条被批准方案作为有效目标；
- 阻塞主张与认证/配置要求出现在同一句或同一局部语境；
- 示例、代码块、行内代码、引号内容、Markdown 表格和假设性方案不作为当前阻塞事实。

若当前目标是分析、设计、解释或诊断，自纠偏不得注入 `deliver_changes`、不得继续无关工具链，也不得让已经完成的正确回答降级为折叠步骤。

## Applicable Harnesses

- Spec Harness：CF-SCC-R1..R19，并与 CF-CCE-R1..R25 的连续性契约联合验证。
- Compatibility Harness：旧 settings、旧 completion state、旧会话 hydration。
- Viewport Harness：1366×768、800×600 下状态卡与输入区不重叠。
- Observation Harness：真实 App 中 Full access 诊断、Execute 恢复和取消路径。
- Payload Harness：状态卡不得泄漏完整命令、凭据、内部 prompt 或大段 tool result。
- AI Collaboration Harness：独立架构/UX 审查、失败测试、最终验证结果。
- Release Harness：PR+CI、安装包与发布 App 精确版本证据。

## 完成边界

只通过 Rust 或 React 单测不算完成。必须证明 Full access 不再改变回合意图、segment 内恢复 guard 真实生效但不会终止用户目标、内部恢复与跨段续跑期间用户可见进度、历史兼容，以及发布 App 的真实会话行为。
