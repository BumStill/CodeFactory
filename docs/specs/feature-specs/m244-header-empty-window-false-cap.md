# M244：每周计量套餐被响应头里的空窗口误判为已达上限

## 背景
2026-11-06，用户的 ChatGPT 套餐改为按周计量，每周上限设为 50%。M241、M242 两个会话各只调用了一次模型，一秒后 ChatGPT 就被判为「已达订阅用量上限」，而实时查询显示周用量 32%、`over_cap=false`。路由记录是 `subscription_quota_cap:{"resets_at_ms":null}`。

## 根因
`parse_codex_usage_headers` 把重置时间为 0 的响应头窗口记成「刚好已重置」的 5 小时窗口。`evaluate_cap` 对已重置的服务端窗口按 100% 处理，这是 M164 的保守规则。响应头读数写在 `base_url` 键上，而界面和实时查询看的是端点名键，所以两边结论不同。

响应头的原始值没有留存，这条根因由代码和 `resets_at_ms:null` 推断，需按 R3 的记录在真机确认。

## Requirements Traceability
| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| R1 | 窗口长度或剩余时间不大于 0 的响应头窗口不算读数 | `an_empty_header_window_does_not_block_a_weekly_only_plan`、`a_header_window_with_no_time_left_is_not_a_reading`，修复前失败 |
| R2 | 真实读数到达上限时仍然拦下；M164 对过期读数的保守规则不变 | `a_real_header_reading_over_the_cap_still_blocks`、`a_fully_reset_server_snapshot_remains_blocked_until_refreshed` |
| R3 | 路由记录写明拦下的窗口、用量、上限和读数来源 | `subscription_quota_cap` 记录的字段 |

## Applicable Harnesses
Spec Harness、Compatibility Harness、Release Harness、Observation Harness。

## 真机验收
发版安装后，一个使用 ChatGPT 的会话要连续完成多次模型调用，不再出现误判的 `subscription_quota_cap`。如果仍被拦下，用 R3 的字段确认真实窗口和用量。
