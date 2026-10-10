# Prefer 模式回落（U30）

## Test isolation (U32)

| Req ID | Requirement | Minimum evidence |
| CF-PFB-R7 | Model-routing tests don't affect each other's results: any subset, any order, any degree of parallelism produces the same outcome (including the Windows CI environment) | The full `--lib` model_transport tests run 20 times in a row (including shuffled order / multi-threaded) without a single failure; record the command and results in the PR |
| CF-PFB-R8 | If the production code holds endpoint state that can be shared across sessions (e.g. availability/cooldown), keep the semantics the product needs, but tests must be able to isolate it; also state in the PR whether sharing across sessions is intended product behaviour and why | PR explanation + tests |
| CF-PFB-R9 | The two failing tests above, and the PRs they block (#589, #590), pass on the combined state of the latest main | Combined local run + CI results |

## Background and user decision
"Prefer" means use the user's chosen model first and automatically switch to the next one when it can't be used, so the work isn't interrupted. The user picked the order and doesn't want to watch over or step in by hand.

现场证据（2026-10-09 17:23:42–17:25:17，session dd3779b3，policy `prefer`）：ChatGPT 端点连续 6 次返回 `ENDPOINT_UNAVAILABLE`（`error sending request for url (https://chatgpt.com/backend-api/codex/responses)`），`model_route_attempts` 里只有 chatgpt 记录，**没有一次 deepseek 或 openrouter 尝试**；任务最终被判 `technical_recovery_exhausted`，接近完成的工作丢失。17:28 chatgpt.com 已恢复，本应早已切到 DeepSeek。

## Decision
In prefer mode, once the preferred endpoint is judged unavailable, the current task continues on the next available endpoint in order, and goes back to the preferred one later once it recovers. Fixed mode is unchanged.

## Requirements Traceability

| Req ID | Requirement | Minimum evidence |
| --- | --- | --- |
| CF-PFB-R1 | In prefer mode, when the preferred endpoint is unavailable (can't connect, timeout, 5xx, sign-in expired, quota used up or rate limited), the same task switches to the next available endpoint in order (chatgpt → deepseek → openrouter) within a bounded time (at most one brief retry), and the task doesn't fail | Table-driven tests: each kind of unavailability → the next request goes to the next endpoint; one synthetic end-to-end proving the task continues |
| CF-PFB-R2 | Fall back only when the next endpoint is actually available; when all endpoints are unavailable, tell the user plainly which models were tried and when it will try again (consistent with the existing behaviour for brief outages) | Test: all three unavailable → plain-language state, no "recovery exhausted" |
| CF-PFB-R3 | After falling back, new turns try the preferred endpoint again; when quota is used up, don't keep hitting the preferred endpoint before the quota resets | Test: preferred recovers → next turn goes back; quota error → don't retry within the reset window |
| CF-PFB-R4 | Switching doesn't break the conversation in progress: context, tool-call history, and the model-specific fields still in effect are converted correctly, and the request isn't rejected (e.g. reasoning/response fields that can't be carried across providers) | Integration tests: chatgpt→deepseek and deepseek→openrouter each carry an in-progress conversation with tool calls |
| CF-PFB-R5 | The session states in plain words which model is currently answering, and that it switched because the preferred one was unavailable; usage records go to the endpoint that actually served the request | Component test + passes the internal-vocabulary guard; usage record test |
| CF-PFB-R6 | Fixed mode never falls back (regression) | Test |

## Applicable Harnesses
Spec Harness; Compatibility Harness (existing session/route records); Observation Harness (route attempts can be traced per endpoint); AI Collaboration Harness.

## Test matrix
- Unavailability types: can't connect / timeout / 502-503-504 / 401 sign-in expired / 429 or quota used up.
- Order: chatgpt down → deepseek; chatgpt + deepseek down → openrouter; all down.
- Going back: preferred recovers → back on the next turn.
- Conversation in progress: switching after tool calls have happened, including a thinking/reasoning model.
- Fixed mode: no fallback.

## Constraints
- U25 (#584, merged) changed "brief network/service outages wait and resume instead of failing". Make it consistent with that: in prefer mode, **if there's an available fallback, switch first; only wait when everything is unavailable**. If you think the two conflict, write a Spec Amendment.
- Don't change the user's endpoint configuration or credentials; tests use synthetic endpoints only.
- Other constraints are as in the workflow template.

## Implementation Notes
（不改变任何 Requirement 或验收标准，只记录实现约束。）

- 缺陷根因：`provider_recovery::record_failure` 在「POST 已准入但拿不到显式响应」（`replay_is_proven == false`）时，把该 attempt 记为 `unknown`、episode 记为 `unknown`，并做出 `DurableWaiting` 决定；`RoutedDesktopModelTransport::complete` 见到 `DurableWaiting` 就直接返回 `PROVIDER_RECOVERY_WAITING`。该等待是 **objective/episode 级**（episode 身份只由 objective 快照决定，与端点无关），并且 `begin_attempt` 会因上一条 attempt 状态 `unknown` 而 fence 掉整条 episode，所以同一目标在任何后续 generation 里都只可能重试首选端点 —— 这正是 6 次 chatgpt、0 次 deepseek 的机械原因。
- 因此回落必须发生在**失败结算处**，而不是在 transport 的 `DurableWaiting` 分支里事后换路：只有当「本轮被证明可安全重放（无输出、无副作用）」且「计划里确有一个当前可用的下一个端点」时，该 attempt 才以 `failed_replayable` 结案并返回 `RetryAfter`，让 `ActiveRouteState::advance_after_failure` 正常推进；不满足这两个条件时保持 U25 的 `DurableWaiting` 等待语义（R2）。
- 「有可用回落」由 `ActiveRouteState::has_available_fallback()` 判定：仅看**当前端点之后**的候选，要求未失败且不在端点冷却期内 —— 与 `advance_after_failure` 的推进方向完全一致，避免出现「判定可回落但实际推不动」的分叉。
- 不可用判定沿用 `classify_provider_failure` 的 `permits_endpoint_failover()`（连接失败/超时/5xx、模型不可用、401、429、配额用尽）；这些类别现在都会把端点写入冷却期（R3 的「配额重置前不要反复打首选端点」在单轮内由冷却期 + `failed_indices` 保证，跨轮由「新一轮先试首选、一次失败即冷却」保证，仍是 bounded）。
- `fixed` 模式下 `settings_for_session_route` 会把端点收敛为唯一一个，计划里不存在回落候选，`has_available_fallback()` 恒为 false（R6）。
- endpoint 冷却窗口沿用既有 `DEFAULT_ENDPOINT_COOLDOWN`（120s），不新增配置项、不改用户端点配置或凭据。

## Fallback when a reply breaks off partway

### Background and evidence（U33，2026-10-09 / 2026-10-10）
- 2026-10-09，session ab2d7b0f（U32 任务，`prefer`，chatgpt/gpt-6-luna）：
  - 20:50:36、20:50:54、21:32:13 连续三次 `TRANSPORT_UNREACHABLE`（`HTTP error: error decoding response body`），`model_route_attempts` 里**一次 deepseek/openrouter 尝试都没有**。
  - objective 从 19:50 到 21:42 停在 `waiting_system / provider_transport_unreachable` 约 2 小时，重试间隔一度到 40 分钟，期间毫无进展；同时 chatgpt.com 与 api.deepseek.com 都是通的。
- #586（U30）留下的已知缺口：「副作用未知时（`!replay_is_proven`）仍停在 objective 级等待，而不回落」。本案例正中该缺口：模型**自己的回复中途断掉**（外部什么都没碰过），却被当成「不能安全重放，等着」。
- 补充证据（2026-10-10 10:20–10:24，M51 会话 4c1fdf4f）：首选 GPT-6.1-Sol（prefer）在一轮中途收到 429 `usage_limit_reached`（plus 套餐 300 分钟窗口，约 3.7 小时后重置）。`model_route_attempts` 三次失败均 `output_started=1`、`side_effect_started=1`，没有回落到 DeepSeek；objective 进入 `waiting_system / provider_overload_budget_exhausted` 原地等几个小时。
- 「额度用完」与「瞬时过载」性质不同：前者重置时间明确且很长，原地等待只会烧掉恢复次数。期望：额度用完时后续请求直接走回落链，或至少明说「GPT 额度到 HH:MM 才恢复，已切到 DeepSeek / 需要你决定」。

### Decision
回复中途断掉、且外部什么都没碰过时，这只是一个「可以重发的模型请求」，按既有回落链换端点重发，不长时间干等；只有工具调用/外部副作用可能已经开始时，才回到「先核对」的谨慎路径。首选端点恢复后，新的一轮仍回到首选。

### Requirements Traceability

| Req ID | Requirement | Minimum evidence |
| --- | --- | --- |
| CF-PFB-R10 | 模型回复中途断掉（解码失败/流中断/连接重置）且**尚未开始任何工具调用或其他外部副作用**时，计为「可重发的模型请求」：prefer 模式下在有界时间内切到下一个可用端点重发同一请求，不长时间等待 | 集成测试：断流 / 解码失败 / 连接重置 → 下一次请求打到 deepseek，任务继续 |
| CF-PFB-R11 | 只有工具调用/副作用可能已经开始时，才回到「先核对」的谨慎路径（维持现状，不得弱化） | 反例测试 |
| CF-PFB-R12 | 所有端点都不可用时的等待时间有上界且可见：会话用平实的话说明下一次尝试会在什么时候发生；超过 N 分钟（取值见 Implementation Notes 并在 PR 说明）没有任何尝试即算缺陷 | 测试 + 平实措辞通过内部术语守卫 |
| CF-PFB-R13 | 首选端点恢复后，新的一轮回到首选端点 | 测试 |

### Applicable Harnesses
Spec Harness；Compatibility Harness；Observation Harness（每一次尝试按端点可追溯）；AI Collaboration Harness。

### Test matrix
- 断流类型：纯文本流中断 / 解码失败（`error decoding response body`）/ 连接重置。
- 已起副作用：流中断前已经吐出 `ToolCallStart` → 不换路（反例）。
- 额度用完（429 `usage_limit_reached`）：有回落可用时不烧预算，直接换路。
- 等待上界：所有端点不可用 → 下一次尝试时间 ≤ N 分钟且用平实语言说出来。
- 回归：fixed 模式仍不换路；首选恢复后新一轮回到首选。

### Constraints
- 与 U25（#584 短暂断网等待）和 U30（#586 prefer 回落）保持一致；冲突时写 Spec Amendment。
- 其余约束同工作流模板。

### Implementation Notes
（不改变任何 Requirement 或验收标准，只记录实现约束。）

- 根因：`RoutedDesktopModelTransport::complete` 在 POST 之后的 `Retryable` 分支里用**本轮 `output_started`**（任何 SSE 事件，含纯文本 `TextDelta`）加上 turn 级 `turn_uncommitted_output` 判定「不可换路」；而 `TrackingEventSink` 同时把 `turn_uncommitted_output` 也置位。于是「模型已经开始说话但中途断掉」把换路彻底挡住，只能原路重试 —— 这就是 ab2d7b0f / M51 里 0 次 deepseek 尝试的机械原因。
- 判定改为按**副作用**而非「任何输出」：新增本轮 `round_side_effect_started`（只在 `ToolCallStart` / `ToolResult` 置位），换路条件是「本轮没有工具调用」且「本轮开始前 turn 级副作用闩未置位」（R10）；任一为真则保持谨慎路径（R11）。
- 对偶账本（`provider_route_attempts`）仍由 `record_failure` 的 `replay_is_proven = !attempt.side_effect_started && receipt 为空` 决定，纯文本中断因此以 `failed_replayable` 结案，端点进入既有 120s 冷却。
- 429 `usage_limit_reached` 目前被 `is_provider_overloaded`（含 `"429"` 字面）归为 `provider_overload`，在无可用回落时才走严格三次预算并落 `provider_overload_budget_exhausted`；R10 让这类中途失败在 prefer 模式下重新被判定为可回落（有回落即 `RetryAfter`），不再烧预算干等。
- R12 的上界取 **N = 5 分钟**（`MAX_WAIT_BEFORE_NEXT_ATTEMPT_MS`），对 objective 级 `next_observation_at` 统一做上界钳制；用户可见文案由 `src/stores/chatEvents.ts` 的 `modelRouteExhaustedGuidance()` 生成，明说下一次尝试的本地时间（HH:MM），并沿用既有平实措辞守卫。
