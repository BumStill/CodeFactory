# Evolution Agent UX 设计

## 1. 核心用户路径

Phase 0 不先新增大而全的看板，而是让现有 Profile 学习日志、工具门控、自我改进提案首次拥有真实数据。主路径：

1. 用户在真实项目中完成一次包含工具调用的 session。
2. 系统在后台记录成功/失败/拒绝、耗时和脱敏参数摘要。
3. session 结束后产生待审学习或跨会话模式。
4. 用户在现有审核界面查看证据并接受或拒绝。
5. 接受的知识/偏好影响后续 session；工具门控仍需单独点击启用；Evals 不自动运行。

## 2. 信息层级

- 第一层：状态和结论——成功、失败、拒绝、证据数、候选去向。
- 第二层：脱敏证据——工具名、参数摘要、错误摘要、耗时、session 来源。
- 第三层：原始详情——仅后续受权限控制打开，Phase 0 不增加原文浏览器。

不向用户展示 ClickHouse、HDBSCAN、OTel Collector 等部署名词；这些不是本地产品首期操作对象。

## 3. Review 行为

- `Accept` 只执行当前明确类型的动作：memory 写项目记忆、preference 写偏好。
- pattern 的 Harness/Evals/知识去向在统一候选模型完成前只能作为建议，不得假装已经落地。
- Skill 始终先生成 disabled proposal，用户预览后再启用。
- 工具门控只允许 `allow -> ask`，且必须由用户点击。
- 产品代码、PR、部署和发布不在 Phase 0 UI 中提供自动动作。

## 4. 空态与错误态

- 真正没有达到样本门槛：显示需要多少真实调用，不显示“系统健康”。
- 采集失败或 dropped：明确显示数据不完整，禁止用 0 失败率表示正常。
- 普通聊天无 task run：仍可使用会话摘要；不得显示永久空态。
- anonymous：明确“无痕会话不会进入自进化分析”。
- 脱敏命中：详情以 `<redacted>` 展示，不提供绕过按钮。
- pattern 证据徽标只在 `evidence_json.support_unit` 明确声明时显示“session”或“决策”；旧记录缺少单位时统一显示中性的“条证据”，避免把历史调用数/任务数误说成跨会话数。

## 5. Viewport

- 目标：1366×768 与窄窗口。
- Review 主动作在首屏可见；详情可折叠；错误与隐私状态不能只靠颜色。
- 长路径、错误和参数摘要必须换行或截断，不能撑破卡片。
- 真实桌面验证使用已注册的 CodeFactory Dev app wrapper，不能只依赖 jsdom；并行任务存在时使用独立 identifier、端口和数据目录，不抢占共享 wrapper。
- 普通 Dev 验收开始前由 agent 自行启用并保存“信任模式（完全放手）”，不把产品工具授权交给用户；只有权限询问、拒绝、hook cancel 或绕过防护本身是验收目标时才临时切回 ask/deny，结束后恢复完全权限。
- 完全权限只覆盖当前任务内的产品工具调用，不扩大部署、外发、账号、支付、交易、数据删除等权限。

## 6. Phase 0 实地验收

在同一真实项目分别执行：allow 成功、ask 后拒绝、hook cancel、工具返回 error、dispatch error。核对工具卡和 SQLite 的 tool、status、duration、error、cwd、session；重启后仍可追溯。再执行 anonymous 同类动作，确认计数不变。最后从 Profile 运行跨会话分析，并验证它读取真实新轨迹而不是 fixture。

2026-07-14 隔离的 `CodeFactoryEvolutionDev` 已实地走过 allow/error、ask/hook cancel、完全权限无弹窗、进程重启后 done/error 工具卡恢复、两个同项目 session 的跨会话 miner、Profile 人工采纳、写入项目记忆和重复分析去重。真实 miner 展示 `bash` 跨 2 个 session、19 次调用、5 次错误、26%，与数据库一致。anonymous 零持久化沿用本日早先实测；真实 dispatch error 和修复后的 post-mortem 模型候选仍是 Phase 0 剩余证据，不得从单元测试推定通过。
