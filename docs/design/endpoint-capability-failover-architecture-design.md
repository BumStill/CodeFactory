# 端点能力感知自动切换架构设计

> 本文同时描述当前上线切片与目标演进。当前切片已经实现有序候选、凭据/模型可用性
> 筛选、故障分类、同进程 visited/cooldown、同回合切换、实际 route 用量归因和自然
> 持久化提示。`ModelCapabilities` 静态预筛选、跨进程 route episode journal 与完整
> attempt telemetry 是后续演进，不得在本次 release note 中声明已完成。

## 1. 边界与依赖

本能力建立在现有共享 `agent-loop`、桌面 `ModelTransport`、会话连续性 journal 和
Settings endpoint 配置之上。路由恢复属于 transport orchestration，不进入 React
组件，不把 provider 差异散落到聊天 UI。

```text
User turn
  -> RouteEpisode(primary route + eligible snapshot)
      -> ModelTransport.complete
          -> success
          -> classified failure
              -> local deterministic repair / short retry
              -> FailoverCoordinator selects next eligible route
                  -> rebuild provider transport
                  -> continue same root turn
              -> exhausted -> actionable terminal failure
```

现有同 route 自愈继续先执行，例如 context compression、vision placeholder、
`max_completion_tokens` 和 tool-choice 适配。只有这些确定性适配和短退避不能恢复时，
才进入 cross-endpoint failover。

## 2. 规范化 route 与能力

新增或等价表达以下共享结构；字段名可以随实现调整，但语义必须完整：

```rust
struct RouteCandidate {
    endpoint_id: String,
    model_id: String,
    api_style: ApiStyle,
    base_url: String,
    credential_ref: Option<String>,
    capabilities: ModelCapabilities,
}

struct ModelCapabilities {
    tool_calling: CapabilitySupport,
    vision: CapabilitySupport,
    context_window: Option<u32>,
    reasoning_efforts: Vec<ReasoningEffort>,
}

struct TurnRequirements {
    needs_tools: bool,
    has_images: bool,
    estimated_context_tokens: u64,
    requested_reasoning_effort: Option<ReasoningEffort>,
}
```

`CapabilitySupport` 至少区分 `Supported | Unsupported | Unknown`。本轮必需能力为
`Unsupported` 或 `Unknown` 时默认排除，不能用“试一下”承担副作用任务。远端
`/models` 结果可以补充模型目录，但不能覆盖用户显式配置的否定能力，也不能被当作
凭据可用证明。

当前 `CustomModel` 已提供 vision、context window 和 reasoning metadata。tool calling
需要增加明确能力元数据或由受控 provider contract 给出；不能仅靠模型名称猜测。

## 3. 候选快照与顺序

`RouteResolver` 在 root turn 开始时从当前 Settings 生成稳定快照：

1. 当前 session 的首选 endpoint/model；
2. 用户已配置的其它 endpoint 的 active model；
3. 应用用户显式配置的 failover 顺序；若旧配置没有顺序，使用稳定、可预测且测试固定
   的 endpoint id 顺序，不使用 HashMap 遍历顺序；
4. 过滤禁用、凭据不可读、无 active model 和能力不匹配项；
5. 只把 `credential_ref` 传给安全凭据读取层，候选、事件和日志不携带明文 secret。

首版可以把“已配置端点均参与自动恢复”作为兼容默认，同时在设置中提供每端点
`allow_failover` 和有序优先级。ChatGPT OAuth route 不读取 synthetic API key。

候选解析不得修改 `default_endpoint/default_model`。session 可记录本回合最终
`effective_endpoint/effective_model`，但下一回合仍以用户当前首选 route 开始，除非
用户明确更改默认值。

## 4. 故障分类

transport 层输出结构化错误，避免 loop 解析展示字符串：

```rust
enum RouteFailureClass {
    TransientOverload,
    RateLimited,
    NetworkUnavailable,
    AuthUnavailable,
    ModelUnavailable,
    FieldUnsupported,
    VisionUnsupported,
    ContextOverflow,
    PolicyRefusal,
    UserCancelled,
    Fatal,
}
```

分类输入包含 HTTP status、provider error code 和受限的 body signature。截图中的
`HTTP 503` + `biscuit_baker_service_me_circuit_open` 应归为
`TransientOverload`。分类器对未知错误 fail closed 为 `Fatal`，但仍可在它明确属于
当前 route 的 transport failure 时尝试下一合格 route；不得把本地 persistence、
tool runtime 或权限错误误归为 provider failure。

错误展示与诊断分离：

- UI 获得本地化 reason 和结构化 action；
- 日志/journal 保存脱敏 failure class、status、provider code 和 request id；
- provider body 不进入普通 assistant 上下文，避免提示污染和敏感数据泄漏。

## 5. Failover episode 状态机

```text
Active(route A)
  -> LocalRetry(A)
      -> Recovered(A)
      -> Failed(A)
          -> Select(B)
              -> Switched(B) -> Active(B)
              -> Ineligible/Failed(B) -> Select(C)
          -> Exhausted -> FailedVisible
  -> Cancelled
```

episode 至少保存：

```text
root_turn_id
episode_id
candidate_snapshot_digest
active_route
visited_routes[(endpoint_id, model_id, failure_class)]
last_confirmed_message_id
last_confirmed_tool_call_id
status = active | switched | exhausted | completed | cancelled
```

选择前原子写入上一 route 的失败和 `visited_routes`；新 transport 成功接管后再写
`route_switched`。恢复/重启时从 journal 重建 visited 集合，不重新尝试已访问 route。
同一 `(endpoint, model, failure_class)` 在一个 episode 最多一次。候选用尽后只能
进入 `exhausted`，不得重新生成快照形成循环。

## 6. 上下文与工具连续性

共享 loop 继续持有规范化 `ChatMessage`、tool definitions、permission policy 和 cancel
flag。切换 transport 时：

- 从规范化 history 为目标 `ApiStyle` 重新编码 payload；
- 保留已经持久化的 assistant tool call 与 tool outcome 配对；
- 从 `last_confirmed_tool_call_id` 之后继续，不重放成功工具；
- 目标 provider 不接受来源 provider 的 reasoning trace 时，将其视为不可执行的
  历史说明或省略，不伪造成用户消息；
- 重新执行 context budget 计算；若目标窗口不足则候选不合格，而不是截断关键 tool
  outcome；
- permission、hook、delivery ceiling 和用户取消状态不因 route 切换重置。

切换发生在 model call 边界，不能中断正在执行的 tool 后把其状态视为未知。工具已经
发出但没有持久化 outcome 时，先按 continuity 的 interrupted/reconcile 规则处理，
不能直接在新模型上重试。

## 7. 事件与持久化

新增或等价事件：

- `route_retrying`：当前 route 做短退避；默认只更新同一状态行；
- `route_switching`：包含 source/target display name 和本地化 reason；
- `route_switched`：目标 transport 已成功接管；
- `route_exhausted`：包含结构化 exclusions 和 actions；
- `route_recovered`：首选 route 自愈成功，不制造切换记录。

journal 保存脱敏记录：

```text
episode_id, root_turn_id, endpoint_id, model_id, attempt_order,
failure_class, http_status, provider_code, elapsed_ms, outcome, created_at
```

禁止保存 API Key、OAuth token、完整请求/响应 body、未脱敏 prompt 或工具参数。
usage/cost 必须按实际执行每次 model call 的 route 归属；失败请求若 provider 没返回
usage，不得记为零成本事实，只能记 `usage_unknown`。

hydration 将 route 事件聚合回所属真实用户回合，最终 effective route 可审计但不创造
新的用户消息或手动记忆入口。

## 8. 可行动终态

`route_exhausted` 的结构化 payload 至少包含：

```ts
type RouteExhausted = {
  primaryFailure: LocalizedRouteFailure;
  exclusions: Array<{
    endpointId: string;
    modelId?: string;
    reason: "missing_credential" | "missing_model" | "capability_mismatch" |
            "already_failed" | "disabled";
  }>;
  preservedWork: boolean;
  actions: Array<"retry_primary" | "open_endpoint_settings">;
};
```

`retry_primary` 创建新的 episode 并只由用户触发；`open_endpoint_settings` 定位到具体
故障 endpoint。凭据或登录修改成功后，下一次重试重新读取 Settings 与安全存储。

## 9. 兼容与迁移

- 旧 Settings 没有 capability/failover 字段时保守推导：显式已知能力可参与，必需能力
  为 unknown 的候选不参与；不得因升级自动写入虚假的能力支持。
- 旧 session 没有 route journal 时继续使用原 endpoint/model，不伪造历史切换。
- 回滚版本可忽略新 route 事件，原 messages 和 tool outcomes 仍可读取。
- 匿名会话不写数据库，但内存 episode 仍执行同样的候选、visited 和取消契约。
- headless/benchmark 与 desktop 共享分类器、候选策略和 visited 规则；凭据加载由各
  surface adapter 实现。

## 10. 安全与观测

- 凭据 probe 只判断可读取，不把 secret 放入 route snapshot、事件或测试快照。
- 不做周期性付费健康检查；真实请求失败才触发 failover。
- trace 必须能回答“为什么从 A 切到 B”“为什么 C 被排除”“是否重复工具”，但不泄漏
  provider body。
- route 切换次数、恢复率、exhausted 分类和额外延迟进入产品观测；质量分或自动调优
  不进入首版决策。
