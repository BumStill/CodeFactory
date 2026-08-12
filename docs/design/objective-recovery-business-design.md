# Objective Recovery Control Plane：业务设计

## 背景与问题

CodeFactory 已在 delivery、provider fast path 和 updater 中拥有局部恢复机制，但产品仍以 chat turn、task attempt、tool call、workflow job 为终点。技术状态一旦跨模块或跨进程，用户就会收到“继续”“重试”“重新发送”“回到对话处理”等恢复动作，实际承担调度器职责。

这会造成四类业务损失：

1. 用户离开后任务失去 owner，无法兑现“搞定/全部完成”的授权承诺；
2. 同一目标可能出现重复消息、重复 PR、重复发布或验收降级；
3. 心跳、绿色测试、transport Done 与真正业务完成混淆，降低信任；
4. 技术问题被包装成人工门禁，增加监督成本并阻断规模化无人值守执行。

## 用户承诺

用户只负责：

- 定义目标和验收边界；
- 授权可执行范围；
- 提供系统无法推导的不可替代核心输入；
- 决定无安全默认、不可逆且改变实质业务结果的选项；
- 显式取消或拒绝某项副作用。

CodeFactory 负责把等待、provider、CI、远端对账、分支、权限通道、工具失败、任务验收、浏览器配对、发布管线与进程重启持续推进到目标终态。

## 产品结果

- 用户不再靠“继续”“？”或追问停止原因来推动执行。
- 相同目标只有一个 objective、一个 revision 轨迹和一个 canonical delivery identity。
- 阶段性回复、stream 结束、turn settled 与业务完成严格分离。
- 技术恢复耗尽进入可观测 remediation queue，不伪装成用户 blocker。
- 必要输入只请求一次；输入满足后自动续接，不要求重述任务。
- 任何恢复先观察 receipt，再决定是否重放，外部副作用至多一次。

## 产品范围

| Surface | Objective identity | Durability promise |
| --- | --- | --- |
| Project / Quick chat | session + root turn + typed scope | 跨 stream、窗口、App restart |
| Workspace Task / subagent | session + task/subagent binding | 跨 attempt、route、App restart |
| Tool / permission / terminal / browser | objective + action/resource generation | receipt 对账后自动续接 |
| Delivery / release / update | objective + repo/head/canonical remote identity + ceiling | 跨 CI、merge、workflow、安装重启 |
| Anonymous chat | 进程内临时 identity | 为保护隐私不落盘；开始前明确“不支持跨 App 重启恢复” |

## 决策政策

| 情况 | Owner | 产品行为 |
| --- | --- | --- |
| 网络、provider 429/5xx、CI、测试、分支、timeout、重启 | system | 自动退避、切 route、修复、对账或排队 remediation |
| permission timeout/channel close | system | 结束本次等待但保留 objective；重建通道并自动续接 |
| 新的高风险/权限扩大授权 | user authorization | 一次结构化授权；授权完成即自动续接 |
| 首次 OAuth、2FA/CAPTCHA、不可替代文件 | user input | 一次合并请求；完成后自动续接 |
| 无安全默认的不可逆业务结果 | business owner | 一次结构化决策；选择后自动续接 |
| 明确拒绝/取消 | user | 绑定 action signature 或终止 objective；不得绕过 |
| 系统能力耗尽 | platform | `failed_internal/platform_incident` + remediation owner；不回交“继续” |

## 成功指标与发布阈值

正式版最近 24 小时必须满足：

- 非业务阻断用户回交率：0%；
- 已授权 next action 重复确认：0；
- 同 objective 可避免 user reprompt：0；
- ownerless 超过 lease：0；
- duplicate external side effect / PR / release：0；
- false complete 与 requested acceptance downgrade：0；
- core input 同 objective 请求次数：不超过 1；
- `needs_business_decision` 结构完整率和精确率：100%；
- 自动恢复成功率、P50/P95 恢复时间与 domain 分布可查询。

任何零容忍指标非零，release 只能标记未满足，不能用隐藏错误、缩小范围或人工补录绕过。

## 非目标

- 不自动代选不可逆业务结果。
- 不绕过 hard deny、明确拒绝、required checks、签名、发布或 live verification。
- 不盲重放已有输出或副作用未知的 provider/tool/browser/release 请求。
- 不承诺所有外部故障立即成功；承诺目标始终有可审计 owner、下一步和安全边界。
- 不引入独立遥测服务；首期复用本地 Tauri + SQLite 与现有 release evidence。

## 交付策略

1. 先建立全域 Objective、DecisionEnvelope、CompletionArbiter 和 durable supervisor；
2. 再接入 permission、scheduler、provider/auth、browser、delivery/release adapters；
3. 统一 UI 投影并删除旧人工技术恢复契约；
4. 用跨进程故障注入、CodeFactoryDev 和正式安装版验证；
5. 只有生产指标达到阈值后，产品结论才能从“局部满足”改为“满足”。
