# M155：重启续跑必须走同一套路由，且前置失败必须结算

## 现场（v1.91.24，2026-10-22 17:49）

会话 `db4a6…` 在回合中途被重启续跑。续跑选到了**没有凭据**的 OpenRouter 端点，
传输层立刻返回 `AUTH_MISSING: openrouter 尚未配置凭据`，而回合此前已经被准入
（`thin_session_state=running` + `chat_turn_state=working`）。错误只留下日志，
回合永远停在「运行中」，零模型调用，用户下一条消息被排在一个死回合后面。

ChatGPT（会话正在用的订阅端点）已过留余量阈值、DeepSeek 在额度窗口之外——
这两条都属于「软偏好」，都不应该把路让给一个根本没有任何凭据的端点。

## 结论（本轮实现）

**R1 路由单一入口。** `commands::chat::resolve_turn_route()` 是唯一入口，内部固定
`settings_for_session_route()` → `resolve_route_plan()` 两步。正常回合、
`resume_thin_running_session()`、`resume_chat_objective_inner()`、用户「继续/重试」
四个入口全部改走它。入口之间无法再用不同的参数口径选出另一套端点。

**R2 续跑前置失败必须结算。** 两条续跑路径在**模型调用之前**解析路由失败时，
不再裸 `?` 返回，而是走 M151 R10 同一条结算缝
`running_guard.settle_now(Some(&reason))`：回合原子结算为等待（路由/额度类可重试 →
`waiting_system` 语义的等待；需要用户动作 → 用户可见的等待），运行控制置
`completed`，下一条「继续」立即可准入。失败原因原文写进会话与日志（R3）。

**R1 收紧：无凭据端点不进计划。** `resolve_route_candidates()` 现在把
`endpoint_requires_api_key() && key_ref.is_none()` 的端点直接从候选里剔除，并记入
`excluded`（人话原因：`<endpoint>：尚未配置凭据`）。它既不能做首选，也不能被
留余量/失败切换选中——M155 的 `AUTH_MISSING` 因此在选路阶段就不可能发生。

## 回归测试

- `restart_route_keeps_the_usable_endpoint_and_never_targets_a_credential_less_fallback`
  ——现场 fixture（ChatGPT 会话端点 + DeepSeek 有凭据但窗口外 + OpenRouter 无凭据）：
  续跑与正常回合得到同一份计划；首选是 ChatGPT；OpenRouter 被排除并给出原因。
- `resume_entry_points_use_the_single_turn_route_entry_and_settle_failures`
  ——结构回归：两个续跑入口都调用唯一入口，且失败分支含 `settle_now(Some(&reason))`。
- `route_failure_before_the_first_model_call_settles_the_turn_as_a_wait`
  ——DB 级行为：以现场那句 `AUTH_MISSING` 收敛后，目标不是 `active`，运行控制
  `completed`，即「续跑失败 → 继续立即执行」。

## 未完成（后续）

- **R5 跨进程 smoke**：`~/.codefactory/agent-tools/verify-liveness.sh` 的等价断言
  （重启后最终状态 ≠ running、给出回答）尚未写进仓库的跨进程 smoke，也未接入
  release 流水线。现有 `--history-session-smoke` 父/子进程框架是落点。本轮为紧急
  修复，先保证路由与结算逻辑正确并有用例覆盖；真机重启验收仍由编排方发版后按
  R4 步骤核对。

## 附带修复：关闭浏览器的退出证明改为有界重试

PR #842 的首轮 CI 在 `check-rust`（Windows runner）失败于
`tools::browser_session::tests::thin_browser_open_reaches_driver_without_receipt`，
错误是「browser close and forced kill could not prove process exit」。与本单的路由修复
无关，是同一条易抖路径：`teardown()` 只问一次退出证明——`close`+`wait` 各 5 秒，
拿不到就判失败。`kill` 只是请求终止，回收可以晚于它返回；负载高的 runner 上这就是
「主机慢」被误报成「关不掉」。

现在 `teardown()` 在首次询问失败后，于 `EXIT_PROOF_GRACE`（15 秒）窗口内以 2 秒
超时 + 250 毫秒间隔反复向 `Browser::wait()` 要证明：拿到就成功，整个窗口都拿不到
才保留原错误。**没有放宽为无证据的成功**——「必须证明进程退出」这条不变量不变。
