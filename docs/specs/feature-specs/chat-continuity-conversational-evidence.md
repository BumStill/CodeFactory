# 会话连续执行与自然工具证据

## Requirements Traceability

| Req ID | 要求 | 验证 |
| --- | --- | --- |
| CF-CCE-R1 | 用户目标不得因 Interactive/Execute/Autonomous 的内部 iteration 数耗尽而结束；内部预算只形成可续跑 segment | Rust failure-first loop sequence |
| CF-CCE-R2 | segment 边界必须先持久化最后工具 outcome 和 continuity checkpoint，再自动调度下一 segment；未完成时不得发成功 `Done` | Rust journal ordering + event assertions |
| CF-CCE-R3 | 自动续段沿用同一 session、root turn、目标、权限、累计 recovery、失败签名、wall-clock 和取消状态，不把续跑伪装成新用户请求 | Rust integration + SQLite assertions |
| CF-CCE-R4 | 连续无材料进展必须换策略并最终收敛为有证据的 Blocked；不得以“30/80 轮上限”作为用户可见阻塞 | policy unit + user-visible copy negative assertion |
| CF-CCE-R5 | transport 异常、工具异常、spawned agent panic、abort、应用退出和续段调度失败必须落库为 completed/blocked/cancelled/failed/interrupted 之一，不得留下永久 running | panic/restart integration |
| CF-CCE-R6 | watcher 捕获后台 task panic 后 2 秒内发送可见中断事件、释放 running/cancel owner，并保留诊断日志 | Rust async test + real app |
| CF-CCE-R7 | 重启 hydration 遇到无活跃 owner 的悬空工具尾部时，5 秒内显示可恢复中断；不得继续显示旧计时或假运行 | SQLite fixture + real app |
| CF-CCE-R8 | “继续执行”复用原 root goal 和检查点，从最后确认边界继续，不重复执行已成功的非幂等工具 | resume integration + side-effect counter |
| CF-CCE-R9 | 助手正文是主阅读线；成功工具默认为无全周边框、无阴影的行内证据，运行/权限/失败使用轻背景或左侧状态线 | component + compiled CSS + real app |
| CF-CCE-R10 | 相邻三个及以上例行成功工具可折叠；不得跨助手正文、失败、权限或用户消息分组，展开后顺序与审计内容不变 | timeline component tests |
| CF-CCE-R11 | 工具折叠态不解析大 diff/完整输出，摘要有界且不泄漏 prompt、凭据或未脱敏参数 | lazy/payload tests |
| CF-CCE-R12 | 主题 token 支持 Tailwind `<alpha-value>`；生产 CSS 必须真实生成工具证据使用的 border/background opacity 类 | production CSS assertion |
| CF-CCE-R13 | 历史 hydration 按真实用户回合重组 narration、tool replay、continuity 和 final；同一回合密度与 live timeline 一致 | hydration/store + component fixture |
| CF-CCE-R14 | 聊天气泡不再显示手动 `Remember` 入口；长期项目记忆由会话后学习自动物化，Profile 保留查看/编辑入口；step/notice/checkpoint/interrupted/rejected 均不出现手动记忆控件 | component + learning materialization regression |
| CF-CCE-R15 | 连续性与工具状态可键盘操作、读屏可感知且不只依赖颜色；减少动态效果设置有效 | accessibility component + real app |
| CF-CCE-R16 | 浅色/深色、1366×768/800×600 下无黑框墙、无整页横向溢出，正文保持第一视觉层 | four-viewport real app evidence |
| CF-CCE-R17 | 普通短会话、超长历史、排队消息、匿名会话、completion recovery、sticky-scroll 和工具权限语义不得回归 | compatibility matrix |
| CF-CCE-R18 | PR+CI、main、公开安装包和精确版本真实 App 验收完成前保持 `not live` | Release Harness evidence pack |

## Primary User Paths

### 连续执行成功路径

用户明确要求完成一项超过单 segment 预算的实现。达到内部边界时，CodeFactory 保存工具结果和检查点，用一句低干扰状态说明“已保存当前进度，正在继续处理”，自动启动下一 segment。后续 segment 完成剩余修改、测试和验证，聊天只保留一个用户目标和一个最终回复。

### 中断恢复路径

Agent 在成功编辑文件后 panic 或应用退出。数据库已记录工具 outcome；重启后 CodeFactory 识别该 root turn 没有活跃 owner 和合法终态，在原位置显示“执行意外中断，已保留完成内容”。安全条件满足时自动恢复，否则提供“继续执行”。恢复从最后确认边界开始，不重复编辑或重复外部写操作。

### 自然对话路径

助手先解释正在检查的问题，随后出现低对比的搜索/读取证据行；助手给出判断，再显示编辑和测试证据；最后用正常回复交付结论。20 个成功工具不会形成 20 个黑框，相邻例行项可折叠，失败仍直接显示首行原因。

### 历史恢复路径

用户重启并打开长会话。UI 按真实用户回合恢复同一条对话流，工具证据密度与 live 相同；技术 message row 不制造额外大间距。聊天气泡不显示手动记忆入口，悬空回合显示恢复状态而非假完成。

## Applicable Harnesses

- Spec Harness：CF-CCE-R1..R18 逐项追踪。
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
| Rust panic | spawned chat future 在工具完成后 panic | `npm run cargo:shared -- test --manifest-path src-tauri/Cargo.toml chat_task_panic -- --nocapture` | 2 秒内 interrupted 落库与事件；running/cancel owner 清理 |
| Rust resume | 非幂等 fake tool 计数后模拟进程重启 | `npm run cargo:shared -- test --manifest-path src-tauri/Cargo.toml continuity_resume -- --nocapture` | 计数保持 1；从 tool outcome 后续跑；最终终态唯一 |
| Frontend event | checkpoint/resumed/interrupted/terminal 乱序与迟到尾页 | `npm test -- --run src/stores/chatEvents.test.ts src/stores/chatEvents.gate.test.ts src/stores/chatEvents.longSession.test.ts` | 同一 root turn 定点更新；迟到 hydration 不覆盖 live segment |
| Tool UI | success/running/permission/error 与 6 个连续成功项 | `npm test -- --run src/components/ToolCallCard.test.tsx src/components/ToolCallCard.error.test.tsx src/components/ToolCallCard.lazy.test.tsx src/components/MessageList.timeline.test.tsx` | success 无全边框；attention 有文字；分组边界正确；折叠不解析 diff |
| History/Memory | 一个回合被持久化为多条 assistant/tool/notice 行；postmortem 生成安全 memory 候选 | `npm test -- --run src/components/MessageList.gate.test.tsx src/components/MessageList.renderIsolation.test.tsx src/stores/chatEvents.segments.test.ts` + Rust learning materialization tests | hydration 后仍是一条自然流；气泡无 Remember；安全 memory 自动写入且 marker 去重 |
| Theme build | `border-border/25`、`bg-surface-1/30` 等 opacity token | `npm run build` 后检查 `dist/assets/*.css` | 生产 CSS 含 alpha 规则；浅色不回退为不透明 `#1e293b` 黑框 |
| Browser | 20-tool fixture，短流、长流、失败、用户上翻 | `npm run dev`，用真实浏览器执行 1366×768 与 800×600 | 无横向溢出；正文优先；长执行折叠；上翻不强拉回 |
| Dev App | 低 segment budget、panic hook、强制退出/重启 fixture | `pnpm tauri dev` 或 `scripts/install-dev-app-wrapper.sh` 启动 `CodeFactoryDev.app` | 自动续段、panic 可见、重启 5 秒内可恢复；成功/边界路径各一次 |
| Release App | 从公开 macOS/Windows 产物安装精确版本 | 标准 release workflow 后在产物上重走 Dev App 路径 | 版本匹配；真实 app 四视口/主题和连续性全部通过 |

测试名在实现时必须落到上述筛选关键字，避免矩阵变成不可执行的占位描述。若新增独立 headless 脚本，应加入 `package.json` 并由 CI 调用。

## Evidence Pack Requirements

证据包至少包含：

- 30 轮边界 fixture 的事件序列、root turn、segment index 和最终终态；
- panic/abort 后数据库 continuity 记录、用户可见提示与 owner 清理证据；
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
- jsdom/Vitest、Rust 单测、`npm run build`、PR 绿色或 workflow 成功均不能单独证明完成。
- 只有公开发布版本的真实 App 通过连续执行、中断恢复、自然工具证据、四视口/主题和自动记忆验收后，状态才可从 `not live` 改为 `live`。
