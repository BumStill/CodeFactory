# Prefer 模式回落（U30）

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
