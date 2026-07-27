# 模型运行时控制面

## Requirements Traceability

| Req ID | 要求 | 验证 |
| --- | --- | --- |
| CF-MRC-R1 | Settings 只保存新会话默认；会话保存独立 endpoint/model/policy | migration + session command tests |
| CF-MRC-R2 | 旧会话确定性迁移为 `fixed`，不因升级自动跨供应商 | migration tests |
| CF-MRC-R3 | fixed/prefer/auto 语义符合设计，切换只在下一轮生效 | planner + real App |
| CF-MRC-R4 | 回合开始冻结不可变计划，当前运行不受设置或会话修改影响 | concurrency regression |
| CF-MRC-R5 | OAuth start 立即返回共享 flow/auth URL；自动打开失败不丢流程 | coordinator tests |
| CF-MRC-R6 | Settings 与历史会话提供打开、复制、取消、过期重建入口 | component + real App |
| CF-MRC-R7 | ChatGPT 401 映射 `AUTH_EXPIRED`，同步账号状态并显示会话恢复动作 | classifier/reducer/component |
| CF-MRC-R8 | 授权成功不自动重放有输出、工具意图或副作用不明的回合 | replay guard tests |
| CF-MRC-R9 | 凭据按候选惰性读取；未使用 DeepSeek 不触发其 Keychain lookup | broker call-count tests |
| CF-MRC-R10 | 同一 key_ref singleflight/cache；超时后不立刻产生第二个 OS prompt | broker concurrency tests |
| CF-MRC-R11 | legacy Keychain 成功授权读取后单次迁移；删除清理所有副本 | secrets migration tests |
| CF-MRC-R12 | auth/missing/permission/quota/rate/endpoint/capability 错误结构化区分 | classifier table |
| CF-MRC-R13 | 图片能力参与候选资格；不静默移除附件 | resolver + real App |
| CF-MRC-R14 | 实际 endpoint/model 写入消息、用量和 route attempt，不泄漏 secret | persistence assertions |
| CF-MRC-R15 | Quick/Project/Anonymous/subagent 使用同一策略契约 | adapter tests |
| CF-MRC-R16 | PR、CI、merge、正式 release、公开安装包真实路径前保持 `not live` | Release Harness |
| CF-MRC-R17 | root turn 产生工具或可见输出后，后续模型 round 也禁止跨供应商 | routed transport integration |
| CF-MRC-R18 | 历史 `auth_expired` 回合重载后仍可原地重新验证；恢复后只提示用户明确重发，不自动或一键重放 | hydration + recovery component tests |

## Primary User Paths

### OAuth 自动打开失败

用户在 Settings 点击“重新验证”。系统立即显示等待验证卡片；系统浏览器没有出现。用户
点击“打开验证页面”或“复制链接”，完成授权后 Settings 和原历史会话同时变为已连接。

### 历史会话中恢复

ChatGPT 回合返回 401。会话显示 `AUTH_EXPIRED` 恢复提示，而非“凭据、余额或端点均不可用”。
用户在会话内重新验证，成功后明确点击重试；历史和附件不丢失。

### 会话策略切换

会话 A 从“首选”切为“固定”，会话 B 保持“自动”，Settings 的新会话默认不变。切换时
会话 A 正在执行，当前回合仍使用冻结路由，下一轮才固定。

### DeepSeek 不再反复弹钥匙串

ChatGPT 固定会话发送消息时，route planner 不读取 DeepSeek key。只有用户切到 DeepSeek
或实际 failover 即将使用 DeepSeek 时才读取；一次授权成功后本进程不重复弹窗。

### 图片能力

用户附带图片，当前固定模型不支持视觉。消息和预览保持原样，发送前显示能力不匹配和换
模型入口；系统不删除图片重试。

## Applicable Harnesses

- Spec Harness：CF-MRC-R1..R18。
- Compatibility Harness：旧 DB、旧 Settings、旧 turn_error、Quick/Project/Anonymous/subagent。
- Release Harness：PR、main CI、刻意发版、公开 macOS/Windows 产物。
- Observation Harness：auth state、route attempt、实际 endpoint/model、恢复动作。
- Payload Harness：OAuth URL/token/secret/附件不进入日志和错误证据。
- Viewport Harness：1366×768、800×600、390×844。
- AI Collaboration Harness：规划、架构、QA 独立复核，主实现保持单一变更所有权。

## 可执行验收

| 层级 | 场景 | 必须断言 |
| --- | --- | --- |
| DB | legacy session + ambiguous model | fixed；不静默修复到其它供应商 |
| Session | A fixed ChatGPT；B auto；改 A | B 和 Settings 均不变 |
| OAuth | auto-open error | start 成功返回同一 URL；Open/Copy 可用 |
| OAuth | Settings 发起、会话观察 | 同一 flow；任一页面取消/完成后状态一致 |
| OAuth | shell opener 整体失败 | 手动打开明确失败；复制仍返回同一 URL；不声称已打开 |
| Error | ChatGPT 401 / quota / 429 / 503 | 四种 code 不混淆 |
| Broker | fixed ChatGPT + DeepSeek configured | DeepSeek lookup count = 0 |
| Broker | 两个并发 DeepSeek turn | OS lookup count = 1 |
| Capability | image + no-vision fixed model | 请求数 = 0；附件不变 |
| Replay | auth 恢复前已有 tool call | 无自动重放；显示显式动作 |
| Replay | 上一 round 已有 tool result、下一 round pre-output 503 | fallback hit = 0 |
| UI | 390px 授权卡和模型弹层 | 无横向溢出；动作可见；正文不低于 12px |
| Release | 发布安装包 | 精确版本、账号恢复、策略隔离、Keychain 路径实测 |

## 状态

实现、PR、CI、发布和真实安装包验收完成前：`not live`。
