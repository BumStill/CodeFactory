# P0 执行器存活修复交付证据

Release-Urgency: immediate

## 根因和变更
R1：derive_state 先用 SQLite active/current-process 控制记录筛选，才能查询内存 stop flag。admit_chat_run 先接纳内存回合，再等待 DB/模型设置，登记空窗会把真实回合判 ended。现直接在统一 admission 登记会话级进程内弱引用，完成按 run identity 清除，启动不重建。
R2：启动与心跳统一至少 10 分钟宽限，恢复要求 running 投影、turn/control 末次活动均过期；未知活性保持原状。同态读取不刷新活动时间。
R3：终止事实使旧问题/唤醒失效，新用户消息可启动新回合；恢复不选 ended。

## Req 与测试
- R1/R4 live_executor_without_durable_registration_survives_three_ticks：修复前 Ended != Running，修复后通过。该测试是合成 admission/refresh 检查，phase 标签不是实际模型/工具执行，不能当完整 E2E。
- R2 recovery_protects_fresh_session_even_when_startup_passes_zero_grace、uncertain_running_projection_is_not_ended_or_retimestamped：先失败、后通过。
- R3 recovery_never_reopens_ended_even_with_stale_unfinished_controls：先失败、后通过；terminal_facts_invalidate_old_questions_and_background_work 通过，fixture 约束错误不计行为红灯。
- 当前聚焦 19+1 已通过；前端 pnpm test / pnpm build 已通过。正式 cargo build 成功后，从 /tmp 执行 --thin-executor-liveness-smoke：真实 HTTP SSE 模型等待及真实 bash 工具等待期间共 6 次心跳、12 次启动/心跳恢复扫描，状态保持 running、cancel=false，最终 ended，正确回答及工具结果各落库一次。回执 /tmp/codefactory-executor-liveness-receipt.json。随后按模板执行 workspace 全量回归，退出码 0；正式构建、仓库外功能验收、全量测试按独立步骤顺序完成。
- node scripts/verify-dispatch-bridge-headless.mjs 退出码 0：真实 Chromium + production App/stores/handlers，合成 IPC；两种主题下派单仅发送一次且路由正确，证据 artifacts/m83/headless/receipt.json 与 settings-light.png / settings-dark.png。此证据与正式可执行 smoke 分开，不冒充完整安装版 dispatch E2E。
- R2 新增 recovery_uses_latest_user_activity_not_an_old_control：先失败后通过；近期消息/turn/control 均阻止陈旧控制记录触发恢复，真正续跑前重检。
- R3 补充：未被新用户消息取代的 stop 事实优先于仍在退出的活任务；迟到回答不得覆盖停止事实。

## Harness 与 AI Collaboration
Spec：版本化任务原文；Compatibility：真实 schema 合成 fixture/旧控制记录；Observation：状态、取消信号、时间断言；Release：仅合并授权，不发版；AI Collaboration：已派发独立只读审查，结果未收齐。无 UI 改动。
context scope：thin derive/restart、chat admission、进程内句柄；assumptions：沿用 10 分钟；review point：登记/终止/恢复竞态；validation result：聚焦通过、完整验收未完成。

## 自验缺口
R1：已复现并修复登记空窗。
R2：新近/陈旧孤儿及 ended 排除已测，完整并发压力未测。
R3：stop 旧事实已测，最终回答/连续失败全部排列未独立覆盖。
R4：完整 --dispatch → 模型等待/工具 → tick×3 → 回答 E2E 未执行，不能宣称通过。
R5：dev app 浏览器只读和网页读取实测未执行。真机验收由编排方发版后完成：各自在模型/工具等待期间触发心跳三次，确认 running 保持且最终回答；stop 后重启仍 ended。
§8：尚未合并，无合并确认/分支清理/缓存清理完成声明。

README-Update: reviewed
README-Update-Reason: 内部活性及恢复修复，不改变公开产品承诺。
Scenario-Test: SC-001
