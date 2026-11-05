# M234：端点冷却被静默跳过，会话被挂到另一个端点的额度/预算重置时间上

## 现场（会话 1cd9cecf）
- ChatGPT 在上一轮只出现一次可解析失败（`error decoding response body`），`EndpointHealthRegistry` 按设计把它置入 120 秒冷却——这是 M220 的瞬时退避机制，本意是「一分钟后重试」。
- 下一轮准入时，冷却中的端点被 `is_available() == false` **静默跳过**：既没有留下路由尝试记录，也没有把自己的冷却剩余时间交给等待判定。
- 于是「没有可用模型」的唯一原因变成了其余端点的状态：DeepSeek 当日预算已用完（次日 0 点重置），OpenRouter 无凭据。会话被挂到**次日 0 点**的预算重置上。
- 用户看到的是「当前没有可用模型：DeepSeek 今日预算已用完，11-06 00:00 后继续」，而真实原因是 ChatGPT 一次可以马上重试的瞬时失败。系统在不该睡的地方睡了十几个小时。
- 伴随症状：等待文案里的「N 分钟后重试」是固定值，与真正的唤醒时间（冷却剩余 / 退避档位）不一致；连续重试超过上限时也没有明确的终态说明。

## 要求
- R1：冷却中的端点是一条**会被时间治好**的可用性原因，不是「没有这条路」。准入必须知道它的冷却结束时间，并把「冷却剩余」作为一条瞬时原因参与等待计算，而不是静默跳过。
- R2：等待的唤醒时间与原因必须指向真正出问题的端点。只要有瞬时原因存在，就不得把唤醒时间落在另一个端点的额度/预算重置上；等待文案要写出失败端点（如「ChatGPT 连接出错，N 分钟后重试」），并如实写出还要满足的其它限制。
- R3：冷却跳过必须留下可审计的路由尝试记录（`status: skipped`，`failure_code: endpoint_cooldown:{"until_ms":…}`），冷却结束时间与真实值一致。
- R4：说的等多久就要等多久。文案中的「N 分钟后重试」必须与真实唤醒时间一致：冷却比退避档位更长时按冷却算；旧的固定文案按形状剥离后重写，不再保留过期数字。
- R5：这类等待必须复用 M220 的持久退避——1/2/5/10/30 分钟，同一端点连续 12 次后转成明确的失败提示（说清试过什么、证据仍保留），不得无限静默等待。
- R6：冷却结束后，紧接着的下一轮准入必须重新尝试该端点，而不是继续排到备用端点后面。

## 验收
- T1（R1+R2+R3+R4）：ChatGPT 冷却中 + DeepSeek 当日预算耗尽 + OpenRouter 无凭据的一轮准入，返回模型等待；唤醒时间 ≤ 冷却结束后约 1 分钟且早于 DeepSeek 的次日重置；文案含 `ChatGPT` 与「重试」；路由记录里有带真实冷却结束时间的 skipped 快照；payload 带 `transient_endpoints`、`restriction_resume_at_ms`、`transient_not_before_ms`。
- T2（R6）：冷却过期后下一轮准入把 ChatGPT 重新选为当前路由。
- T3（R5）：同一端点的瞬时等待连续 12 次内每次都继续重试、唤醒不早于冷却结束、文案与实际时间一致；第 13 次转终态，文案含端点名与「12 次」并说明证据保留，且不再排唤醒。
- 回归：`agent::failover`、`agent::usage_windows`（含 `model_wait`）、`agent::thin_state` 的既有 M123/M210/M220 用例保持通过。

Release-Urgency: immediate

## Implementation Notes

- `EndpointHealthRegistry` 新增 `cooldown_remaining()`：`None` 表示现在可用，`Some(剩余)` 表示还需等多久；`is_available()` 改为它的布尔投影，行为不变。
- `ActiveRouteState::admit_model_availability`：冷却中的候选不再静默跳过，而是记一条 `skipped` 路由尝试、向原因集合追加 `WaitCauseKind::Transient`（带真实冷却结束时间），并收集到 `transient_blocked`。
- 等待判定后，把原因拆成两类：瞬时原因（决定重试节奏，取最晚）与其它限制（额度/预算/时段，仅作说明，取最早）。只要存在瞬时原因，就在错误 payload 里带上 `transient_endpoints` / `transient_not_before_ms` / `restriction_resume_at_ms`，让 M220 的 `bound_transient` 接管退避与 12 次上限。
- `bound_transient`：唤醒时间取「退避档位」与「冷却结束」的较晚者（早了只会在冷却里空转），展示分钟数同步取较晚者并限制在 1–60 分钟；文案按形状剥离旧的重试段后再重写，避免出现两个互相矛盾的 N。
- 未改配置、凭据协议与前端文案键：`model_availability_waits` / `wakeup_schedules` 的表结构与使用方式不变。

## Harness 与证据

- Spec / Compatibility Harness（T1–T3 字段级断言 + 既有 M123/M210/M220 用例）、Release Harness（合并交付，不宣称 live）、Observation Harness（路由尝试快照与 `transient_endpoints` 可供审计）、AI Collaboration Harness（context scope：路由准入与模型等待；assumption：冷却只是时间问题的投影；review point：瞬时原因与限制原因的取舍；validation result 见 PR 证据）。本任务不涉及上传、首屏/响应式，故不启用 Payload / Viewport Harness。
- 单执行流完成，未启用并行 sub-agent。
