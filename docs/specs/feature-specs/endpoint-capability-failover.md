# 模型端点自动切换

## 范围

本规格定义首个可上线切片：当当前模型端点在**尚未产生可见输出或工具调用**时发生
可重放的服务故障，CodeFactory 使用本机已经配置、能解析模型且能读取凭据的其它端点
继续同一回合。

能力元数据筛选、跨进程恢复 route episode、结构化“重试原端点/打开设置”按钮和完整
attempt journal 是后续增强，不作为本切片的完成声明。现有 vision/context/tool-choice
确定性兼容逻辑保持不变；本切片不得让这些行为倒退。

## Requirements Traceability

| Req ID | 要求 | 验证 |
| --- | --- | --- |
| CF-ECF-R1 | 每个回合从当前 endpoint/model 开始，并对本机 Settings 生成稳定候选快照；不得使用未配置端点 | resolver unit |
| CF-ECF-R2 | 缺模型、缺凭据或凭据读取失败的端点不进入请求候选；ChatGPT OAuth 不读取 synthetic API key | resolver/credential tests |
| CF-ECF-R3 | macOS Keychain 读取必须离开 async runtime，并对每项并发设置 2 秒防冻结上限；单个坏候选不得卡住整个会话 | timeout regression + real App |
| CF-ECF-R4 | `503/5xx/circuit_open`、429、连接超时/拒绝及 401/403 可触发下一端点；400、context、vision、policy/fatal 不得错误切换 | classifier table tests |
| CF-ECF-R5 | 同端点已有短重试耗尽后，按候选顺序 A→B→C；同一候选每回合最多一次，禁止 A→B→A | routed transport tests |
| CF-ECF-R6 | 一旦产生可见 SSE、tool call 或 tool result，不再跨端点重放，避免混合回答和副作用重复 | partial-output regression |
| CF-ECF-R7 | 切换保持同一 session、root turn、规范化 history、工具结果、权限和 cancel flag | shared-loop integration |
| CF-ECF-R8 | 切换后的 route 驱动 context/reasoning 计算，并按真实 endpoint/model 记录 usage | context + usage attribution tests |
| CF-ECF-R9 | 自动切换只影响当前运行，不修改 `default_endpoint/default_model` | settings immutability assertion |
| CF-ECF-R10 | 成功切换持久化一条自然中文 `turn_notice`；刷新后保持同样的无框低干扰表现 | loop persistence + component tests |
| CF-ECF-R11 | 同端点重试折叠为一个 disclosure；不得堆叠多个醒目 amber 卡片 | reducer/component tests |
| CF-ECF-R12 | 候选耗尽和发送前凭据失败均显示可行动中文说明，技术失败链默认折叠 | store/reducer/component tests |
| CF-ECF-R13 | 匿名会话、桌面会话和 subagent 使用同一候选解析与 routed transport；subagent 不设固定十分钟终止器 | adapter tests + code inspection |
| CF-ECF-R14 | PR、CI、merge、正式 release 和公开产物验收完成前状态为 `not live` | Release Harness |

## Primary User Paths

### 503 自动接管

ChatGPT 请求经同端点短重试后仍返回
`503 biscuit_baker_service_me_circuit_open`。若尚未产生输出，系统改用本机已配置且有
凭据的 DeepSeek active model。同一回合继续，显示：

> ChatGPT / gpt-5.5 暂时不可用，已自动切换到 DeepSeek /
> deepseek-v4-pro，任务继续执行。

最终回复与工具流仍属于原用户消息；用量归因到 DeepSeek。

### 坏凭据候选不冻结

Settings 中存在没有 Key 的 OpenRouter。系统读取该候选时即使 macOS Keychain 阻塞，
也必须在单项 2 秒后排除它；ChatGPT 或 DeepSeek 的正常路径继续。若全部候选都不可用，
对话显示解决办法，而不是一直“运行中”。

### 可见输出后失败

模型已经流出正文或工具意图后连接中断。系统不自动换模型重放，明确结束当前回合，避免
把两个模型的内容拼在一起或重复副作用。

### 用户取消

用户停止生成后，共享 cancel flag 阻止新的模型请求。取消不是 provider 故障，不通过
换端点绕过。

## Applicable Harnesses

- Spec Harness：CF-ECF-R1..R14。
- Compatibility Harness：旧 Settings、ChatGPT OAuth、OpenAI-compatible、匿名/桌面/
  subagent。
- Observation Harness：候选顺序、failure class、实际 effective route、usage attribution。
- Payload Harness：凭据只在内存 route 中使用，Debug/事件/错误均不泄漏 secret。
- Viewport Harness：重试、切换、耗尽信息保持自然聊天密度。
- AI Collaboration Harness：设计、后端、前端由独立角色实现/复核。
- Release Harness：PR+CI、main、公开安装包、精确版本和真实 App。

## 可执行验收矩阵

| 层级 | 场景 | 必须断言 |
| --- | --- | --- |
| Resolver | ChatGPT OAuth + DeepSeek Key + OpenRouter 缺 Key | 顺序为首选在前；仅保留可用候选；无 secret 输出 |
| Keychain | 一个 lookup 永不及时返回 | 其它 lookup 并发完成；整体不冻结；坏候选被排除 |
| Classifier | 503/429/401/400/context/vision/fatal | 只有安全可重放的 route failure 允许 failover |
| Transport | A 503、B 429、C success | A/B/C 单调一次；C sticky；无 ping-pong |
| Partial SSE | A 已输出后断线 | 不请求 B；回合可见失败 |
| Usage | A 无 usage、B success 有 usage | 成功用量记到 B 的 endpoint/model |
| Persistence | B 接管成功后刷新 | `turn_notice` 与 final 顺序不变 |
| Frontend | 两次 retry、一次 switch、一次 exhausted | 一条低对比 retry、一条 13px 自然 switch、技术详情折叠 |
| Subagent | 长任务持续超过旧十分钟边界 | 无固定 wall-clock timeout；只受终态、取消及共享安全门禁约束 |
| Release | 正式 macOS/Windows 产物 | 版本、公开资产、updater metadata 与发布记录一致 |

## 证据包

- 原始生产会话中 `503 circuit_open` 的脱敏时间线；
- macOS `sample` 证明卡点位于 Keychain lookup，以及修复后相同坏配置约 5 秒内返回；
- ChatGPT/DeepSeek 凭据存在性检查，只记录“存在/缺失”，不打印值；
- A→B→C scripted transport、partial SSE、cooldown、usage attribution 测试；
- 真实 Dev App 的成功请求及 SQLite 实际 provider/model 记录；
- retry/switch/exhausted 的组件与 hydration 测试；
- 默认设置修复前后快照；
- PR、required CI、merge SHA、release workflow、公开 assets 和 updater metadata。

## 兼容与安全边界

- 不自动创建、复制、导出或打印 secret；凭据仍由现有 secure store/fallback 管理。
- 不修改用户的默认端点；失败端点仅在当前进程进入 120 秒健康冷却，后续回合仍显示
  自动切换说明。
- 不用 failover 绕过权限、取消、内容政策或工具确认。
- mock、unit test、HTTP 200、PR 绿色或本地 Dev App 都不能单独证明上线。
- 候选能力元数据、route episode 跨重启恢复、结构化修复按钮属于后续规格，不能在本次
  发布说明中冒充已实现。
