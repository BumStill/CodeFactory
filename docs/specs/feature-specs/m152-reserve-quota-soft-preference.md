# M152：留余量与备选可用性（R1 已由 M164 用户决定覆盖）

> 2026-10-23 用户明确决定：五小时/周任一达到 cap_percent（默认 80%）就是硬门禁。无备选必须 waiting_wake，禁止继续 GPT。以下软偏好内容仅保留为历史背景，当前规范与验收以 `m164-hard-quota-cap.md` 为准；原 R2 的凭据和时段过滤继续有效。

## 背景（10-22 09:57 会话 343ebd42）

系统提示「GPT 本周窗口已用 80%，为给你留余量已切到 DeepSeek」，但当时：

- DeepSeek 在时段外；
- OpenRouter 没有任何已保存的凭据（`openrouter` 在可用性列表里却显示 `available: true`）；
- 于是一个也没落成，同一个技术失败连续 4 次
  （`首选端点失败：AUTH_MISSING: openrouter 尚未配置凭据`），会话被结束。

## 根因

1. **规划的备选过滤只看额度**：`resolve_route_plan` 选「下一个端点」时只判断它是否
   订阅额度已满，没有判断时段和凭据，于是把一轮交给一个用不了的路由。
2. **合成凭据引用让缺 key 的端点看起来可用**：`resolve_route_candidates` 为
   API-key 风格端点合成 `codefactory.endpoint.{name}`，`has_usable_credential()`
   因此为真；真正读取 keychain 时才在传输层变成 `AUTH_MISSING`。
3. **"留余量"被当成硬失败**：没有备选时规划直接返回错误；即使走到准入阶段，
   只到 80% 的端点也会被当作不可用而等待/失败。

## 规则

- **R1 软偏好**：`endpoint_usage_windows`/`quota_cap` 的"留余量"上限（未用满 100%）
  是偏好，不是硬门禁。备选不可用时继续使用当前端点，并在会话里说明原因
  （「…当前没有其它可用端点，系统继续使用 GPT…」）。
- **R2 备选可用性**：切换前必须同时满足「有已落盘凭据（`key_ref`）」、
  「当前在可用时段内」、「未到额度上限」。缺凭据的端点不参与路由选择、
  不计入失败次数，也不被描述为可用。
- **R3 真不可用即等待**：所有端点都不可用（含服务端已用满 100%）时进入
  waiting_wake，等到最早可用时间再继续；等待不是技术失败，不得结束会话。
- **R4 可验证**：GPT 85% + 备选时段外/无凭据 → 本轮在 GPT 上跑完；
  其余端点都被跳过的原因逐条出现在等待/路由说明里。

## 实现位置

| 位置 | 变化 |
| --- | --- |
| `src-tauri/src/commands/chat.rs` | 备选必须 `has_usable_credential()` + 时段可用 + 额度未满；无可用备选时 `keep_capped_primary()` 而不是报错；不再合成凭据引用 |
| `src-tauri/src/agent/failover.rs` | 计划构造只把有凭据的端点当交接目标；准入阶段把未用满的额度上限记为 `soft_keep`，仅在所有候选都不可用时启用，并给出说明 |
| `src-tauri/src/agent/quota_cap.rs` | `CapBreach::is_hard_exhausted()`：100% 才是真用满 |

## 测试

- `m152_capped_primary_stays_when_the_fallback_has_no_credential`
- `m152_reserve_capped_primary_is_kept_when_every_alternative_is_blocked`
- `m152_reserve_handover_requires_a_usable_alternative`

## 已知边界

界面里的 usage window 行只表达"时段可用性"（`evaluateWindow`），凭据是否配置由
路由/等待说明（`route_prefilter:missing_credential`）表达；两者不混用同一个字段。
