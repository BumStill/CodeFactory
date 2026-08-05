# 会话执行治理 P0–P1

状态：v1.77.5 已发布；2026-08-05 P0 已完成本地实现和 Dev App 验证，待 PR/CI/正式发布
范围：turn capability、delivery preflight/outcome、completion convergence、task segment、进度快照、sub-agent attempt

## 现场基线

- 正式版近 24 小时：31 条 root 用户输入、685 次工具调用、38 次 gate recovery、40 个 rejected candidate。
- “先出方案不修改”回合仍发生 11 次 `edit_file`。
- 13 次 `deliver_changes` 均返回业务 blocked，但 normalized tool status 均为 done。

## 执行清单

- [x] 正式版轨迹与现有代码/规格复核。
- [x] 业务、架构、UX 设计与 Requirements Traceability。
- [x] Failure-first：ReviewOnly 硬门禁。
- [x] Failure-first：delivery preflight 零副作用与 structured blocked。
- [x] Failure-first：completion recovery 最多一次。
- [x] Failure-first：segment/progress/attempt additive migration。
- [x] P0 实现与 focused verification。
- [x] P1 实现与 reload/viewport verification。
- [x] 真实 CodeFactoryDev 主路径。
- [ ] PR、CI、merge、release artifact 与正式 App 验收。

## 2026-08-01 P1：长等待与工具放大

- [x] 正式版 24 小时轨迹确认：23 个 root 用户回合、373 次工具调用，工具/回合 16.22；9 次工具执行超过 60 秒，最长约 560 秒。
- [x] 根因确认：backend tool future 等待期间没有 activity heartbeat；现有 ProgressTracker 只治理连续只读调用，不治理 root-turn 总放大。
- [x] 业务、架构、UX 增量设计与 CF-SCC-R31..R34。
- [x] Failure-first：长工具心跳、隐私、终态停止、40 次一次性提醒/收敛。
- [x] 共享 loop 与前端进度卡实现。
- [x] 独立 QA 后补：跨 restart/resume/steer 计数与 notice 去重、活动快照写库失败不取消或覆盖工具 outcome、普通工具错误不误写 segment blocked、能力定制收敛提示、reload warning 语义一致。
- [x] focused/full tests、CodeFactoryDev 成功与边界路径。
- [ ] PR、CI、merge 与发布节奏判断。

## 2026-08-05 P0：完成门禁、可恢复等待与交付幂等

- [x] 正式版 24 小时轨迹确认：14 个根用户回合中 5 次需要用户再次推动；18 次
  `deliver_changes` 全部 blocked，主要摩擦是可恢复等待被提前交还、远端读取失败后
  重复交付，以及用户纠正后没有立即改变执行策略。
- [x] Failure-first：第三次纯远端等待不能退化为 terminal blocker。
- [x] Failure-first：gh PR 列表非 JSON、非数组、字段缺失不得当作空列表。
- [x] Rust/前端增加结构化 `waiting`、`RecoveryClass`、`retry_after_ms` 和状态契约校验。
- [x] `deliver_changes` 在同一工具调用内退避重试，复用 agent-loop 30 秒心跳，不新增
  模型请求；等待中取消只发一次 terminal `Done`。
- [x] GitHub/GitLab/gh PR 查询 fail closed；CI observation unavailable 不再伪装成
  check failure，required-rules 读取失败不能证明绿色。
- [x] focused/full tests、CodeFactoryDev 成功与边界路径。
- [ ] PR、CI、merge、release artifact 与正式 App 验收。

本轮验证：`agent-loop` 72/72、delivery 84/84、delivery tool 7/7、前端聚焦
14/14、前端全量 95 文件 485/485、Rust 全量 732 通过/7 显式忽略/0 失败，
`pnpm build`、TypeScript、diff check 与治理基线均通过。CodeFactoryDev 从当前
worktree 以隔离端口启动，窗口标题 `CodeFactory P0 Verification`、URL
`localhost:14201`，历史工具卡、输入区和普通会话路径可见；临时端口覆盖、进程组和
wrapper 指针均已清理，未终止占用 1420 的其他开发会话。结构化 waiting 的状态呈现、
streaming 保持和取消终态由 reducer/component/loop 测试覆盖，未通过修改 dev 数据库
伪造远端等待。

验证结果：`agent-loop` 65/65、SQLite persistence 12/12、前端 92 文件 474/474、`pnpm build`、Rust workspace check、治理基线通过。真实 CodeFactoryDev 绑定当前 worktree 与 DeepSeek v4 Flash：70 秒、100 秒和 65 秒命令均成功；30 秒时同一活动卡显示“命令仍在运行”，100 秒路径捕获到约 1 分钟文案，工具终态后不残留 running 状态。实地发现无结构化 plan 时未展示独立等待原因，补充 failure-first component 回归后修复；热更新后的 warning tone 有单测与 build 证据，但未在短暂 60–65 秒窗口内重新截到实地图。独立 QA 发现的续跑/steer 去重、活动快照失败和假 blocked 均先复现失败再修复，未触碰正式版数据库。

## v1.73.0 continuation 回归

- [x] 正式轨迹确认：ReviewOnly root 中的“继续/开始实施”steer 未更新硬 capability，5 次 mutation 被拒，最终却写为 completed。
- [x] Failure-first：无问号方案后的继续、运行中 capability revision、Git 本地状态变更、blocked 终态、recovery 不重置、未知 MCP fail closed。
- [x] 框架修复：每轮 tool schema 过滤、steer 安全升降级、有效目标统一、结构拒绝真终态。
- [x] 隔离 Dev App 真实路径：只读方案后发送裸“继续”，`state.txt` 从 `VALUE=old` 改为 `VALUE=new` 并通过独立断言；运行中发送“别只审视了，继续实施，把刚才这个方案执行完”后，`state2.txt` 从 `STATE=old` 改为 `STATE=new` 并通过独立断言。
- [x] 实地失败驱动补漏：第一版仅覆盖“继续执行”，真 App 证明“继续实施”仍被 ReviewOnly 拒绝；新增原句回归测试后重载同一候选，原路径通过。
- [ ] PR、CI、v1.73.x 正式发布与公开产物验证。

## AI Collaboration

- Context scope：正式版会话执行链与交付链，不改变普通手工 Git。
- Assumption：明确“发布/上线”构成 Deliver；普通“修复/实现”仅构成 Implement。
- Review point：权限不能扩大意图；业务 blocked 不能再记 done；P1 不删除旧审计数据。
- Validation result：前后端全量测试、Rust workspace check、治理校验通过；真实
  CodeFactoryDev 中只读回合仅调用 `read_file`，33.9 秒一次收敛；诱导
  `touch` 时框架在权限门禁前返回 `denied`，哨兵文件未创建；运行中进度原位
  更新，空数据库在 React StrictMode 下可直接进入一个内存草稿。
