# 模型端点自动切换

## 范围

本规格定义首个可上线切片：当当前模型端点在**当前 root turn 尚未产生可见输出、工具调用
或工具副作用**时发生
可重放的服务故障，CodeFactory 使用本机已经配置、能解析模型且能读取凭据的其它端点
继续同一回合。

能力元数据筛选、会话级模型策略、结构化认证恢复和惰性凭据读取由后续
`model-runtime-control-plane.md` 接管；发生冲突时以该新规格为准。跨进程恢复 route
episode 和完整 attempt journal 仍是后续增强。

route 内的安全切换与 replay latch 继续由本规格定义；候选耗尽、跨进程 ownership、
用户回交与完成语义以 `objective-recovery-control-plane.md` 为准。旧“exhausted 后人工
重试”只代表 approach 已耗尽，不能终止 objective。

## Requirements Traceability

| Req ID | 要求 | 验证 |
| --- | --- | --- |
| CF-ECF-R1 | 每个回合从当前 endpoint/model 开始，并对本机 Settings 生成稳定候选快照；不得使用未配置端点 | resolver unit |
| CF-ECF-R2 | 缺模型、缺凭据或凭据读取失败的端点不进入请求候选；ChatGPT OAuth 不读取 synthetic API key | resolver/credential tests |
| CF-ECF-R3 | macOS Keychain 读取必须离开 async runtime；只在候选真正将被使用时惰性读取，并以 singleflight/缓存避免重复系统授权；单个坏候选不得卡住整个会话 | broker regression + real App |
| CF-ECF-R4 | `503/5xx/circuit_open`、429、连接超时/拒绝可按会话策略触发下一端点；401/403 必须先区分账号过期、缺凭据、权限拒绝和 quota，认证过期不得静默跨供应商；400、context、vision、policy/fatal 不得错误切换 | classifier table tests |
| CF-ECF-R5 | 同端点已有短重试耗尽后，按候选顺序 A→B→C；同一候选每回合最多一次，禁止 A→B→A | routed transport tests |
| CF-ECF-R6 | 当前 root turn 一旦产生可见 SSE、tool call、tool result 或其它副作用，后续模型 round 也不再跨端点重放，避免混合回答和副作用重复 | root-turn replay regression |
| CF-ECF-R7 | 切换保持同一 session、root turn、规范化 history、工具结果、权限和 cancel flag | shared-loop integration |
| CF-ECF-R8 | 切换后的 route 驱动 context/reasoning 计算，并按真实 endpoint/model 记录 usage | context + usage attribution tests |
| CF-ECF-R9 | 自动切换只影响当前运行，不修改 `default_endpoint/default_model` | settings immutability assertion |
| CF-ECF-R10 | 成功切换持久化一条自然中文 `turn_notice`；刷新后保持同样的无框低干扰表现 | loop persistence + component tests |
| CF-ECF-R11 | 同端点重试折叠为一个 disclosure；不得堆叠多个醒目 amber 卡片 | reducer/component tests |
| CF-ECF-R12 | 候选耗尽和发送前凭据失败均产生 typed decision：技术耗尽进入 durable remediation；只有确认不可替代凭据/额度时聚合一次 core input。技术失败链默认折叠且无人工重试 CTA | store/router/reducer/component tests |
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

Settings 中存在没有 Key 的 OpenRouter。系统使用 ChatGPT 时不得预读 OpenRouter
Keychain；只有真正前进到该候选时才读取。读取阻塞或被拒绝时排除它，并显示结构化解决
办法，而不是一直“运行中”或在后续回合重复弹系统授权。

### 可见输出后失败

模型已经流出正文或工具意图后连接中断。系统不自动换模型重放；stream 可以关闭，但
objective 转入 receipt 对账/安全 checkpoint 续接，避免把两个模型的内容拼在一起或重复副作用。

### 用户取消

用户停止生成后，共享 cancel flag 阻止新的模型请求。取消不是 provider 故障，不通过
换端点绕过。

## Applicable Harnesses

- Spec Harness：CF-ECF-R1..R14。
- Compatibility Harness：旧 Settings、ChatGPT OAuth、OpenAI-compatible、匿名/桌面/
  subagent。
- Observation Harness：候选顺序、failure class、实际 effective route、usage attribution。
- Payload Harness：凭据只在内存 route 中使用，Debug/事件/错误均不泄漏 secret。
- Viewport Harness：退避、切换、system-owned 耗尽信息保持自然聊天密度且无人工恢复 CTA。
- AI Collaboration Harness：设计、后端、前端由独立角色实现/复核。
- Release Harness：PR+CI、main、公开安装包、精确版本和真实 App。

## 可执行验收矩阵

| 层级 | 场景 | 必须断言 |
| --- | --- | --- |
| Resolver | ChatGPT OAuth + DeepSeek Key + OpenRouter 缺 Key | 顺序为首选在前；仅保留可用候选；无 secret 输出 |
| Keychain | 一个 lookup 永不及时返回 | 其它 lookup 并发完成；整体不冻结；坏候选被排除 |
| Classifier | 503/429/401/400/context/vision/fatal | 只有安全可重放的 route failure 允许 failover |
| Transport | A 503、B 429、C success | A/B/C 单调一次；C sticky；无 ping-pong |
| Partial SSE | A 已输出后断线 | 不请求 B；stream 关闭但 objective 显示正在对账/续接，不产生用户技术动作 |
| Usage | A 无 usage、B success 有 usage | 成功用量记到 B 的 endpoint/model |
| Persistence | B 接管成功后刷新 | `turn_notice` 与 final 顺序不变 |
| Frontend | 两次 retry、一次 switch、一次 exhausted | 一条低对比 retry、一条 13px 自然 switch、system owner/下次观察可见、技术详情折叠且无 CTA |
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

- 不导出或打印 secret；凭据由 CredentialBroker 按候选惰性读取并管理缓存/fallback。
- 不修改用户的默认端点；失败端点仅在当前进程进入 120 秒健康冷却，后续回合仍显示
  自动切换说明。
- 不用 failover 绕过权限、取消、内容政策或工具确认。
- mock、unit test、HTTP 200、PR 绿色或本地 Dev App 都不能单独证明上线。
- 候选能力元数据仍由模型控制面演进；route episode 必须接入 CF-ORC 的跨重启 supervisor。
  结构化技术重试按钮已被取代，不得在发布说明或 UI 中作为恢复契约。
