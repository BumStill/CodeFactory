# 模型运行时控制面架构设计

## 分层

```text
Settings defaults
  -> SessionModelConfig(endpoint_id, model_id, policy)
  -> immutable TurnExecutionPlan
  -> capability eligibility
  -> lazy CredentialBroker
  -> routed transport
  -> typed runtime/auth event
  -> settings or in-session recovery UX
```

## 会话模型配置

`sessions` 新增：

```text
endpoint_id TEXT
model_policy TEXT NOT NULL DEFAULT 'fixed'
```

允许策略值仅为 `fixed | prefer | auto`。现有 `model_id` 继续保存会话首选模型。

迁移顺序：

1. 添加列；
2. 对 `endpoint_id IS NULL` 的会话，用模型与当前端点目录做确定性匹配；
3. 唯一匹配时写入端点；
4. 无唯一匹配时使用迁移时的默认端点，但只有该端点确实包含该模型才写入；
5. 全部旧会话保持 `fixed`；不能安全解析的会话在发送前返回
   `MODEL_ROUTE_UNRESOLVED`，不隐式改模型。

新会话复制创建时的 Settings 默认：

```text
default_endpoint -> sessions.endpoint_id
resolved default/selected model -> sessions.model_id
default_model_policy -> sessions.model_policy
```

Settings 新增可选 `default_model_policy`，缺失时兼容为 `prefer`。会话内更新使用新的原子命令
`update_session_model_config(session_id, endpoint_id, model_id, policy)`，不得写 Settings。

## TurnExecutionPlan

回合开始时从 SessionModelConfig 构建不可变快照：

```rust
struct TurnExecutionPlan {
    session_id: String,
    preferred_endpoint: String,
    preferred_model: String,
    policy: ModelPolicy,
    required_capabilities: CapabilitySet,
    candidates: Vec<RouteDescriptor>,
}
```

`RouteDescriptor` 只包含端点名、模型、base URL、API style、key ref 和能力，不携带 secret。
`fixed` 只生成一个候选；`prefer` 先首选再兼容候选；`auto` 按能力、认证状态、健康状态和
稳定顺序生成。当前回合不观察后来发生的会话配置修改。

候选只在以下条件同时满足时允许前进：

- policy 不是 `fixed`；
- 失败类别允许跨端点；
- 尚未产生可见正文；
- 尚未产生工具调用、工具结果或其它外部副作用；
- 下一候选满足本轮能力要求；
- 账号不是可在原路由轻量恢复的 `reauth_required`。

这些边界是 root-turn 级 latch，不能在每次 `complete()`/每个模型 round 开始时重置。
只要本 root turn 曾经执行工具或产生可见输出，后续 round 的发送前失败也不得跨供应商。

## AuthCoordinator

进程内 `AuthCoordinator` 管理至多一个 ChatGPT 授权流：

```text
idle
  -> awaiting_browser(flow_id, auth_url, expires_at)
  -> exchanging
  -> completed(account)
  -> failed(code, recoverable)
  -> expired
  -> cancelled
```

命令：

- `codex_login_start`：新建或返回仍有效的共享流程；
- `codex_login_open`：再次请求系统打开同一 URL；
- `codex_login_status`：轮询/刷新阶段；
- `codex_login_cancel`：关闭 listener 并结束流程；
- `codex_account_status`：返回 `ready | refreshing | reauth_required | missing`。

callback listener 和 token exchange 在后台任务中运行。前端无需保持某一个组件挂载；
Settings 和会话恢复组件都只凭 `flow_id` 观察相同状态。OAuth state、PKCE verifier、
code 和 token 不写数据库、不进入前端日志。

`open_browser` 的返回值只表示系统调用被接受。`codex_login_start` 即使自动打开失败也
返回 flow 和 URL，并把 `browser_open_error` 作为非终止诊断。

## 结构化故障

运行时失败统一映射为：

```text
AUTH_EXPIRED
AUTH_MISSING
CREDENTIAL_ACCESS_REQUIRED
CREDENTIAL_ACCESS_DENIED
QUOTA_EXCEEDED
RATE_LIMITED
ENDPOINT_UNAVAILABLE
MODEL_CAPABILITY_MISMATCH
PAYLOAD_UNSUPPORTED
MODEL_ROUTE_UNRESOLVED
ROUTE_EXHAUSTED
```

Stream error 事件增加 `code`、`endpoint_id`、`recoverable`，保留兼容的 `message`。
`AUTH_EXPIRED` 触发会话内重新验证动作并同步 Settings 的账号状态；不得被归入 quota，
不得自动切换到其它供应商掩盖账号失效。runtime adapter 同时保存 `objective_id`、
`resume_cursor`、`output_started`、`side_effect_started` 与 receipt 引用；授权完成后由
RemediationSupervisor 先对账再续接，不能依赖新的用户消息。

授权成功只解除 auth wait，不等于允许整回合 replay：零输出/零副作用可继续当前模型请求；
已有 tool outcome 从其后的 cursor 继续；副作用未知先进入只读 reconcile。三条路径都保持
同一 root turn/objective，并由 replay fence 保证副作用至多一次。

## CredentialBroker

CredentialBroker 接口：

```text
get(key_ref) -> ready(secret) | missing | access_required | denied | unavailable
invalidate(key_ref)
```

实现约束：

- route plan 不含 secret；
- 仅在 transport 第一次使用候选时读取；
- 同一 `key_ref` 的并发读取 singleflight；
- 成功值只在进程内缓存，不写日志；
- 超时不能丢弃仍在运行的钥匙串任务后立刻重复启动；
- macOS legacy Keychain 成功读取后可一次性写入 0600 recovery copy；
- 删除凭据同时清理 Keychain、recovery copy 和进程缓存；
- 非当前候选不能因为“为 failover 做准备”而提前触发系统授权。

## 兼容与可观测性

- 旧 StreamEvent 客户端仍能读取 `message`；新字段可选。
- `model_route_attempts` 是 route attempt 的持久化真相源，至少包含
  `id/root_turn_id/session_id/endpoint/model/policy/status/failure_code/output_started/
  side_effect_started/created_at/completed_at`；不记录 secret、授权 URL 或完整请求体。
- 成功消息与用量归因实际 endpoint/model。
- 匿名、Quick、Project 和 subagent 共用相同的会话策略解析；匿名会话只把配置放内存。
- 回合错误持久化结构化 code；旧 `turn_error` 文本仍按兼容规则显示。
