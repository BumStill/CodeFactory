# M185（P0 热修，阻断全部对话）：v1.91.45 升级后新回合全部失败 `no such column: m.seq`

## 证据（10-27 02:15，本机从 v1.91.44 升到 v1.91.45）
- 每个新会话首回合失败，thin=waiting_user，question=`任务未能启动：Database error: error returned from database: (code: 1) no such column: m.seq。请检查模型凭据或项目配置后重试。`；0 次模型调用。编排方验证会话 e05197d3/1578abf8/9f85b17e 全部如此。
- 引入者：#892（MEM-A，稳定消息 seq/revision + 影子回执）。已有数据库升级后没有 `messages.seq` 列（启动日志历来有 `sqlx::migrate reported migration 1 was previously applied but has been modified; falling back to idempotent ensure_schema`——即真实环境走 ensure_schema 而不是新 migration）。CI/测试用新库，所以没发现。
- 编排方已回滚本机到 v1.91.44 并 pin；v1.91.44 在被 v1.91.45 碰过的库上工作正常。
## 需求
- R1 在 ensure_schema（真实升级路径）里幂等补齐 MEM-A 需要的所有列/表/索引（seq、revision 等），并回填已有消息的 seq（稳定、单调，按现有排序）；对旧版本可兼容（旧版本忽略新列）。
- R2 任何查询依赖新列前，启动自检确认 schema 完整；不完整则自动补齐，补不齐则给明确错误而不是每回合失败。
- R3 回归测试（先失败）：**用 v1.91.44 时代的真实形状数据库**（含「migration 1 被修改」状态）启动 → 新回合成功、seq 已回填；空库同样成功；从 v1.91.45 坏状态再启动也能修复。
- R4 CI 加「旧库升级」门禁：用固定的历史 schema 快照启动并跑一轮对话，作为 PR hard gate，防止以后同类问题。
- R5 发版后编排方真机：升级 → 新会话正常回复。
## 约束
- task-template；合成 fixture（可从 schema 生成，不含真实数据）；授权交付（ceiling: merged）。PR 标题前缀 fix:，立即发版。

Release-Urgency: immediate

## Implementation Notes
- 交付上限为合并；独立发版任务及安装版真机验收由编排方负责。
- Applicable Harnesses：Spec、Compatibility、Release、Observation、AI Collaboration。
- R1–R3 使用生产 connect 和 load_agent_history 接口断言旧库校验和冲突后的序号、版本及重开幂等性；R4 将增加固定历史快照与 CI 硬门禁；R5 不以本地测试冒充真实应用验收。
