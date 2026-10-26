# M178 回合进行中的故障切换不得落到无凭据端点，也不得把「没有可用模型」计成技术失败

状态：实施中（v1.91.37 现场回归，含 M164 #853）
关联：`m123-no-model-wait`、`m155`（凭据门禁）、`m164`（硬上限矩阵）、`endpoint-capability-failover`

## 1. 用户看到的问题（证据）

会话 `aeba516d`（M174，`chatgpt/gpt-6.1`，thin 会话）在 17:42 首轮动手就撞 ChatGPT 五小时窗口上限：

- ChatGPT：本五小时窗口达上限（用户 85% 硬上限），当时无余量落点；
- DeepSeek：不在允许使用时段（14:00–18:00），17:42 属于时段外；
- OpenRouter：**没有可用凭据**。

回合内的故障切换仍然把这一轮交给 OpenRouter，传输层立刻 `AUTH_MISSING: openrouter 尚未配置凭据`。
此后同一条技术失败连续三次，会话以「已结束：连续三次…」收场，而正确结果是**等到 18:00 自动继续**。

M164 #853 已经保证「候选列表只含有凭据端点」，但修的是**候选构造**这一条入口；回合中段的
**耗尽路径**仍然把「所有候选都因为可用性原因不可用」当成普通传输失败，于是它绕过了等待语义。

## 2. 需求

- **R1（唯一过滤 + 静态门禁）**：故障切换链（`push_fallback`）只允许由 `commands/chat.rs` 里的
  规范候选解析器构造；其他位置的辅助调用（标题、摘要、vision）只能构造单候选。任何新增的
  直接构造点在测试里会失败，必须回到规范入口。
- **R2（耗尽即等待，不结束）**：一个回合把所有候选都试完/跳过之后，如果原因是**可用性**
  （缺凭据、不在使用时段、达订阅硬上限），结果必须是 `waiting_wake`：
  - 唤醒时间 = 候选中**最早**的可用时间；没有已知时间时用有界（约 1 分钟）自动重试；
  - **不计技术失败**（连续失败计数保持 0），会话不再 `ended`；
  - 界面给一句人话原因（含每个端点为什么不可用、什么时候自动继续），不出现 `AUTH_MISSING`
    这类内部错误码；
  - 已保存进度与目标，唤醒后是**续跑**原任务，不是重开一个回合。
- **R3（跨进程可验证）**：失败发生在回合中段时，结算仍由落盘的 thin 状态 + 已登记的
  `model-availability-<session>` 唤醒行决定；进程重启后从同一份落盘状态恢复并自动继续。
- **R4（凭据不是时段的附庸）**：使用时段/额度被忽略（`ignore_windows`）时，凭据检查
  依然生效——「忽略时段」不是「允许无凭据路由」的授权。

## 3. 主路径

用户开会话 → 首轮撞上限 → 系统说明「ChatGPT 达上限、DeepSeek 18:00 起可用、OpenRouter 需保存凭据」
→ 行停在 `waiting_wake` 并登记 18:00 唤醒 → 到点系统自动重试并原任务续跑。

## 4. 实现要点

- `ActiveRouteState::exhaustion_availability_error`：把「耗尽」翻译成结构化的
  `MODEL_UNAVAILABLE:{resume_at_ms, notice}`（复用 M123 的编码），前提是至少有一个候选
  因可用性原因不可用；纯传输失败仍走 `exhausted_error` 原文案。
- 传输层两处 `advance_after_failure` 的 `None` 分支改用 `exhaustion_error`。
- `thin_state::settle_error`：`ModelUnavailable` 无论有没有明确恢复时间都结算为
  `waiting_wake`（无时间时按 1 分钟有界重试），并写入 `reason`；`after_no_model` 不再返回空
  reason。

## 5. 测试矩阵

| 用例 | 断言 |
|---|---|
| `m178_midturn_exhaustion_waits_for_the_earliest_window` | 耗尽错误可解析出 DeepSeek 窗口的 `resume_at_ms`，notice 有人话原因、无 `AUTH_MISSING` |
| `m178_missing_credential_is_never_surfaced_as_a_failure_code` | notice 说明「需要保存凭据」，不含内部错误码 |
| `m178_missing_credential_waits_and_does_not_count_a_technical_failure` | `settle_error` 后 state=`waiting_wake`、reason 非空、`consecutive_failures`=0、落盘唤醒行 due=`resume_at` |
| `m178_restart_resumes_from_the_persisted_wait` | 用同一 DB 重新 `load` 得到 `waiting_wake`，唤醒行仍在（跨连接/跨进程同一份状态） |
| `m178_failover_chains_are_only_built_by_the_canonical_resolver` | 源码扫描：`push_fallback(` 只出现在 `agent/failover.rs` 与 `commands/chat.rs` |
| `admit_availability` 忽略时段时仍拒无凭据端点 | 既有 m164 矩阵用例 |

## 6. Applicable Harnesses

Spec Harness、Compatibility Harness（错误编码沿用 `MODEL_UNAVAILABLE:`）、Observation Harness
（`waiting_wake` + 唤醒行）、Release Harness（合并后按日常批量切版）。
