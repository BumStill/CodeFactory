# 会话连续执行与自然工具证据

本规格定义 segment、timeline 与 hydration；跨 segment/process 的 objective 真相、恢复 owner、用户回交和完成语义以 `objective-recovery-control-plane.md` 为准。turn/stream 的 settled、error 或 interrupted 只能是局部投影，不能把 system-owned objective 写成 blocked/failed/completed。

## Requirements Traceability

| Req ID | 要求 | 验证 |
| --- | --- | --- |
| CF-CCE-R1 | 用户目标不得因 Interactive/Execute/Autonomous 的内部 iteration 数耗尽而结束；内部预算只形成可续跑 segment | Rust failure-first loop sequence |
| CF-CCE-R2 | segment 边界必须先持久化最后工具 outcome 和 continuity checkpoint，再自动调度下一 segment；未完成时不得发成功 `Done` | Rust journal ordering + event assertions |
| CF-CCE-R3 | 自动续段沿用同一 session、root turn、目标、权限、累计 recovery、失败签名、wall-clock 和取消状态，不把续跑伪装成新用户请求 | Rust integration + SQLite assertions |
| CF-CCE-R4 | 连续无材料进展必须换策略并最终收敛为有证据的 Blocked；不得以“30/80 轮上限”作为用户可见阻塞 | policy unit + user-visible copy negative assertion |
| CF-CCE-R5 | transport 异常、工具异常、spawned agent panic、abort、应用退出和续段调度失败必须关闭局部 owner 并落库为 settled/interrupted projection，同时把未完成 objective 转入 waiting_system/remediation；不得留下永久 running 或技术 blocked | panic/restart integration |
| CF-CCE-R6 | watcher 捕获后台 task panic 后 2 秒内发送可见系统恢复事件、释放失效 running/cancel owner、排队 remediation 并保留诊断日志 | Rust async + supervisor + real app |
| CF-CCE-R7 | 重启 hydration 遇到无活跃 owner 的悬空工具尾部时，5 秒内显示 system-owned 恢复状态；30 秒内 claim identity 完整的 objective，不继续显示旧计时、假运行或人工恢复动作 | SQLite fixture + real app |
| CF-CCE-R8 | supervisor 自动复用原 root goal 和检查点，从最后确认边界继续，不重复执行已成功的非幂等工具；用户主动“继续”只用于批准方案/新 steer，不是技术恢复依赖 | resume integration + forbidden-CTA + side-effect counter |
| CF-CCE-R9 | 助手正文是主阅读线；成功工具默认为无全周边框、无阴影的行内证据，运行/权限/失败使用轻背景或左侧状态线 | component + compiled CSS + real app |
| CF-CCE-R10 | 相邻三个及以上例行成功工具可原位聚合，但当前 objective 未 completed/cancelled 时不得按固定 segment 阈值整体折叠；turn error、waiting_system 或 platform incident 都仍显示恢复状态。只有业务 completed 或显式 cancelled 后才可收束较早过程。不得跨助手正文、失败、权限或用户消息分组，展开后顺序与审计内容不变 | timeline component tests + objective transition real app |
| CF-CCE-R11 | 工具折叠态不解析大 diff/完整输出，摘要有界且不泄漏 prompt、凭据或未脱敏参数 | lazy/payload tests |
| CF-CCE-R12 | 主题 token 支持 Tailwind `<alpha-value>`；生产 CSS 必须真实生成工具证据使用的 border/background opacity 类 | production CSS assertion |
| CF-CCE-R13 | 历史 hydration 按真实用户回合重组 narration、tool replay、continuity 和 final；同一回合密度与 live timeline 一致 | hydration/store + component fixture |
| CF-CCE-R14 | 聊天气泡不再显示手动 `Remember` 入口；长期项目记忆由会话后学习自动物化，Profile 保留查看/编辑入口；step/notice/checkpoint/interrupted/rejected 均不出现手动记忆控件 | component + learning materialization regression |
| CF-CCE-R15 | 连续性与工具状态可键盘操作、读屏可感知且不只依赖颜色；减少动态效果设置有效 | accessibility component + real app |
| CF-CCE-R16 | 浅色/深色、1366×768/800×600 下无黑框墙、无整页横向溢出，正文保持第一视觉层 | four-viewport real app evidence |
| CF-CCE-R17 | 普通短会话、超长历史、排队消息、匿名会话、completion recovery、sticky-scroll 和工具权限语义不得回归 | compatibility matrix |
| CF-CCE-R18 | PR+CI、main、公开安装包和精确版本真实 App 验收完成前保持 `not live` | Release Harness evidence pack |
| CF-CCE-R19 | 长任务以结构化 plan event 提交有界步骤、等待原因和计划变化；进度条展示当前/下一步，百分比只取 `completed / total` 并标明来源 | tool/event/store/component |
| CF-CCE-R20 | 终态后 5 秒内形成结果快照；结果视图、完整过程切换和证据化重新总结只引用最终回复、plan 与真实工具 evidence | component + headless/real app |
| CF-CCE-R21 | 时间估算结合任务阶段、同项目历史 build/test 时长和关联外部 job 状态；相关样本少于 3 个时不展示 | estimator unit + SQLite profile |
| CF-CCE-R22 | 1000 个 plan/tool 事件仍遵守超长会话有界 hydration 和惰性 payload 契约，不造成新的无界数组或大输出复制 | store/perf fixture |
| CF-CCE-R23 | 同一可见会话按 root turn 持久化内部 task segment；每个 segment 至少包含 `segment_id`、`goal_digest`、`status`、`checkpoint`、`handoff`、开始/结束时间。新目标只加载当前 segment 与有界 handoff，不回灌旧 recovery/tool 噪音 | SQLite migration + context fixture + restart |
| CF-CCE-R24 | 每个 root turn 的实时进度是一个可覆盖快照，至少包含 `phase`、`current_step`、`next_step`、`waiting_reason`、`updated_at`、`elapsed_ms`。重复微更新保留审计事件但 UI/store 不追加同义 assistant message | reducer coalescing + hydration + 100-event fixture |
| CF-CCE-R25 | `task_run` 是逻辑子任务，retry 是 `task_attempts` 的 append-only 记录。每个 attempt 保存 ordinal、sub-session、状态、失败码、时间和 evidence；UI 只显示一张任务卡并可展开 attempts，空 child 也必须成为失败 attempt | additive migration + scheduler + component |

## Primary User Paths

### 连续执行成功路径

用户明确要求完成一项超过单 segment 预算的实现。达到内部边界时，CodeFactory 保存工具结果和检查点，用一句低干扰状态说明“已保存当前进度，正在继续处理”，自动启动下一 segment。后续 segment 完成剩余修改、测试和验证，聊天只保留一个用户目标和一个最终回复。

### 中断恢复路径

Agent 在成功编辑文件后 panic 或应用退出。数据库已记录工具 outcome；重启后 CodeFactory 识别该 root turn 没有活跃 owner 和合法终态，在原位置显示“已保留完成内容，系统正在恢复”。身份和 receipt 可证明时自动恢复；外部副作用未知时先只读对账；平台暂不可用时保持 remediation owner 和下一次观察。恢复从最后确认边界开始，不重复编辑或外部写操作，也不要求用户发送技术恢复消息。

### 自然对话路径

助手先解释正在检查的问题，随后出现低对比的搜索/读取证据行；助手给出判断，再显示编辑和测试证据；最后用正常回复交付结论。运行中的回合始终保持连续阅读线；20 个成功工具不会形成 20 个黑框，相邻例行项可原位聚合，失败仍直接显示首行原因。终态到达后，较早过程才收束到展开入口。

### 历史恢复路径

用户重启并打开长会话。UI 按真实用户回合恢复同一条对话流，工具证据密度与 live 相同；技术 message row 不制造额外大间距。聊天气泡不显示手动记忆入口，悬空回合显示恢复状态而非假完成。

## Applicable Harnesses

- Spec Harness：CF-CCE-R1..R25 与 CF-ORC-R1..R21 联合追踪。
- Compatibility Harness：旧 SQLite、旧 completion state、Interactive/Execute/Autonomous、匿名会话、队列与 recovery。
- Observation Harness：segment 接管耗时、panic 反馈、重启恢复、stream 终态和进程 owner。
- Payload Harness：大 diff、长 stdout、文件参数、凭据和 continuity 摘要脱敏。
- Viewport Harness：浅/深主题的 1366×768、800×600，工具展开、长摘要、输入区和 sticky-scroll。
- AI Collaboration Harness：独立架构、实现和 QA 角色；记录假设、review point 与验证结果。
- Release Harness：PR CI、main CI、macOS/Windows 产物、精确版本和真实 App。

## 可执行验收矩阵

| 层级 | 先失败场景 | 执行命令/步骤 | 必须断言 |
| --- | --- | --- | --- |
| Rust policy | transport 连续 30 轮都返回工具调用且 completion 未满足 | `npm run cargo:shared -- test --manifest-path src-tauri/crates/agent-loop/Cargo.toml iteration_boundary -- --nocapture` | 第 30 轮后为 checkpoint/continue，不是空 `Done`；第 31 轮可继续 |
| Rust journal | 最后一轮工具成功后触发 segment 边界 | `npm run cargo:shared -- test --manifest-path src-tauri/Cargo.toml continuity_checkpoint -- --nocapture` | tool outcome row 先于 checkpoint；root turn/segment 游标正确 |
| Rust panic | spawned chat future 在工具完成后 panic | `npm run cargo:shared -- test --manifest-path src-tauri/Cargo.toml chat_task_panic -- --nocapture` | 2 秒内 waiting_system/remediation 落库与事件；失效 owner 清理；同 objective 自动接管 |
| Rust resume | 非幂等 fake tool 计数后模拟进程重启 | `npm run cargo:shared -- test --manifest-path src-tauri/Cargo.toml continuity_resume -- --nocapture` | 计数保持 1；从 tool outcome 后续跑；最终终态唯一 |
| Frontend event | checkpoint/resumed/interrupted/terminal 乱序与迟到尾页 | `npm test -- --run src/stores/chatEvents.test.ts src/stores/chatEvents.gate.test.ts src/stores/chatEvents.longSession.test.ts` | 同一 root turn 定点更新；迟到 hydration 不覆盖 live segment |
| Tool UI | success/running/permission/error、6 个连续成功项与超过 10 个 segment 的 active→system-wait→completed 回合 | `pnpm exec vitest run src/components/ToolCallCard.test.tsx src/components/ToolCallCard.error.test.tsx src/components/ToolCallCard.lazy.test.tsx src/components/MessageList.timeline.test.tsx` | success 无全边框；attention 有文字；分组边界正确；system wait 仍显示且无整体折叠入口；objective completed 才收束；折叠不解析 diff |
| History/Memory | 一个回合被持久化为多条 assistant/tool/notice 行；postmortem 生成安全 memory 候选 | `npm test -- --run src/components/MessageList.gate.test.tsx src/components/MessageList.renderIsolation.test.tsx src/stores/chatEvents.segments.test.ts` + Rust learning materialization tests | hydration 后仍是一条自然流；气泡无 Remember；安全 memory 自动写入且 marker 去重 |
| Theme build | `border-border/25`、`bg-surface-1/30` 等 opacity token | `npm run build` 后检查 `dist/assets/*.css` | 生产 CSS 含 alpha 规则；浅色不回退为不透明 `#1e293b` 黑框 |
| Browser | 20-tool fixture，短流、长流、active→terminal、失败、用户上翻 | `pnpm dev`，用真实浏览器执行 1366×768 与 800×600 | 无横向溢出；正文优先；active 长回合不整体折叠；terminal 后才收束；上翻不强拉回 |
| Dev App | 低 segment budget、panic hook、强制退出/重启 fixture | `pnpm tauri dev` 或 `scripts/install-dev-app-wrapper.sh` 启动 `CodeFactoryDev.app` | 自动续段、panic system-owned 可见、重启 30 秒内自动 claim；无人工继续 CTA；成功/边界路径各一次 |
| Release App | 从公开 macOS/Windows 产物安装精确版本 | 标准 release workflow 后在产物上重走 Dev App 路径 | 版本匹配；真实 app 四视口/主题和连续性全部通过 |
| Structured plan | 首次计划、步骤推进、等待、增删步骤无 change reason | `pnpm test -- --run src/stores/chatPlan.test.ts src/components/TurnProgress.test.tsx` | revision 有序；当前/下一步正确；计划变化有理由；百分比来源明确 |
| Result snapshot | completed/partial/failed 与 1000-event turn | `pnpm test -- --run src/components/TurnResultSnapshot.test.tsx` | 5 秒内本地形成；完整过程可切；重新总结不调用模型；证据有界 |
| Time estimate | 0/2/3+ 历史样本、build/test、external job | `pnpm test -- --run src/lib/turnEstimate.test.ts` + Rust timing profile tests | 少于 3 不显示；区间和样本来源确定；不输出伪精确 ETA |

测试名在实现时必须落到上述筛选关键字，避免矩阵变成不可执行的占位描述。若新增独立 headless 脚本，应加入 `package.json` 并由 CI 调用。

## P0–P3 映射

| 优先级 | Req IDs | 完成定义 |
| --- | --- | --- |
| P0 | R10 | objective active/system-wait 平铺，completed/cancelled 才收束（PR #235 的 turn 终态语义由 CF-ORC 扩展） |
| P1 | R19、R22 | 结构化执行路线、紧凑进度、等待/变更 |
| P2 | R10、R13、R20、R22 | 结果快照、结果/过程切换、证据化重新总结 |
| P3 | R21 | 有来源的时间区间；数据不足不展示 |

## Evidence Pack Requirements

证据包至少包含：

- 30 轮边界 fixture 的事件序列、root turn、segment index 和最终终态；
- panic/abort 后数据库 continuity/objective remediation 记录、用户可见 system owner 与自动 claim 证据；
- 非幂等 fake tool 在重启恢复前后只执行一次的计数；
- live 与 hydration 的同一 20-tool fixture 对比；
- 浅色/深色 × 1366×768/800×600 截图，包含成功、运行、失败和中断；
- 生产 CSS 中 opacity token 的编译结果，不以 jsdom className 断言替代；
- 聊天气泡无手动 `Remember` 控件的组件断言，以及安全 memory 候选自动写入/去重的文件证据；
- PR、CI、merge、release workflow、公开 artifact URL/校验和、安装后精确版本。

## 兼容与发布边界

- 不把内部 segment 数、iteration 数或 context refresh 次数展示为用户任务上限。
- 不以自动续跑为由绕过工具权限、用户取消或不可逆决策确认。
- 不删除旧消息和工具审计数据；旧数据库必须可升级、可回滚读取。
- `task_segments`、`turn_progress_snapshots` 与 `task_attempts` 只能 additive migration；旧 `task_runs.sub_session_id` 继续作为最近一次兼容字段读取。
- jsdom/Vitest、Rust 单测、`npm run build`、PR 绿色或 workflow 成功均不能单独证明完成。
- 只有公开发布版本的真实 App 通过连续执行、中断恢复、自然工具证据、四视口/主题和自动记忆验收后，状态才可从 `not live` 改为 `live`。
