# M210：没有可用模型是「等模型可用」，不是「自动重试 2 次仍未推进」

## 背景与用户决定
- v1.91.65（10-30 08:03–08:44）4 个会话（63f6ea09、45191579、beafe825、5ee1876f）收到新任务后
  `model_route_attempts`：chatgpt `skipped subscription_quota_cap resets_at 09:02`，deepseek
  `failed PROVIDER_FATAL`，没有任何 assistant 回合。系统随后两次 `thin-source-repair` 唤醒
  「接续中断回合」，然后进入 waiting_user：「系统自动重试 2 次后仍未推进。请决定换个做法继续
  还是结束？」——用户根本没法「换个做法」，真实情况是「没有可用模型，09:02 GPT 额度恢复后自动继续」。
- 12:00:01 两个会话的 `model-availability-*` 唤醒按时触发（DeepSeek 时段开始），DeepSeek 仍
  `PROVIDER_FATAL`，GPT 仍在 80% 上限（14:05 恢复）。12:00:05 两个会话都被写成 ended
  `orphaned_by_restart`——应用 07:10 启动、12:14 才重启，**没有重启**。12:02 新派单进入
  waiting_user「自动重试 2 次后仍未推进」。
- 用户决定：模型全不可用时一律按 M123/M200 的「等到可用时间再继续」语义处理，模型可用性等待
  不计入自动重试次数、不得转成让用户选做法；`PROVIDER_FATAL` 必须留下服务方原因。

## 决策
1. 模型可用性等待是一等持久事实（`model_availability_waits`），不是靠会话文案推断。
2. 该等待单独计时：`thin-source-repair` 的自愈次数（`MAX_SELF_CONTINUE`）与服务端的
   `model-availability-*` 唤醒互不计数。
3. 唤醒触发后仍无可用模型：重挂**新 id** 的模型唤醒（保持 `waiting_wake`），而不是结束或提问。
4. 没有任何「时间会修好」的原因（全为致命错误）时，用 5 分钟重试窗口继续等待，并把服务方
   摘要写进那句人话里。

## Requirements Traceability
| Req ID | 要求 | 最低证据 |
|---|---|---|
| M210-R1 | 所有候选模型都不可用（额度上限/时段/致命错误）→ 会话 `waiting_wake`，`wake_source` 指向模型可用性唤醒（最早恢复时间），不计入自动重试次数，不得转 `waiting_user` | `m210_model_wait_never_becomes_question`、`m210_derive_state_keeps_model_wait` |
| M210-R2 | 没有可预期恢复时间（全为致命错误）时给用户一句人话：`<供应商> 调用失败：<服务方原因>`，并按 5 分钟窗口自动继续 | `m210_fatal_only_notice_names_provider_reason` |
| M210-R3 | `PROVIDER_FATAL` 持久化服务方 HTTP 状态码与脱敏错误摘要（不含密钥） | `m210_fatal_attempt_persists_provider_status_and_summary` |
| M210-R4 | 夹具：额度上限 + 致命错误组合 → 断言 `waiting_wake`、唤醒落在 `resets_at`、没有 waiting_user 提问 | `m210_quota_cap_plus_fatal_waits_until_reset` |
| M210-R5 | 模型唤醒触发后仍无可用模型 → 重挂下一个最早恢复时间的模型唤醒（新 id），保持 `waiting_wake` | `m210_fired_model_wake_rearms_with_new_id` |
| M210-R6 | 任何路径不得在无重启时写 `orphaned_by_restart`（M206 不变式，纳入全局测试） | `m210_no_orphan_marker_without_restart`、`m210_pre_birth_turn_still_orphaned` |

## Applicable Harnesses
- Spec Harness（本文件 + Req↔测试映射）
- Compatibility Harness：`model_route_attempts` 只做加列（`ALTER TABLE ... ADD COLUMN`），
  旧数据库可以继续读；`model_availability_waits` 为新增表。
- Observation Harness：模型等待在会话里显示「等模型可用（HH:MM）」，状态由 `thin_session_state`
  与唤醒行双证据支撑。
- Payload/Viewport Harness：无新增上传体或布局变更，不适用。

## 测试矩阵
| 场景 | 目标 |
|---|---|
| 额度上限 + 致命错误 | 新派单后 `waiting_wake`，唤醒 `next_at` = resets_at，无提问 |
| 唤醒触发仍无模型 | 新 id 重挂、仍 `waiting_wake`、不累计 `thin-source-repair` |
| 全致命错误 | 人话里出现供应商与服务方摘要；5 分钟后继续 |
| 孤儿判定 | 有模型等待的会话不被写 `orphaned_by_restart`；真正重启前遗留的回合仍然被清理 |
| 路由记录 | `PROVIDER_FATAL` 行带 HTTP 状态与脱敏摘要 |

## Implementation Notes
- 等待事实存 `model_availability_waits(session_id, reason, resume_at_ms, attempts, updated_at)`；
  真实回合成功结算时清除。
- 重挂复用 `persist_model_wakeup`：已有 pending 模型唤醒时原位续期（避免双唤醒），否则用
  `model-availability-<session>`（首个）或 `model-availability-<session>-m210-<n>`（后续）新 id。
