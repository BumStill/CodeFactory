# M137：「立即发版」由派单携带，交付工具自动写 trailer（会话 3 次漏写）

## 证据
- 规格均要求 PR 正文带 `Release-Urgency: immediate`，但 #783（M131）、#800（M136）、#802（重启 P0）正文都没有 → 合并后不自动切版，编排方手动 dispatch。靠模型记忆不可靠。
## 需求
- R1 `--dispatch` 的 create_and_send/send 增加可选字段 `release_urgency: "immediate" | "default" | "hold"`，持久化到会话（会话级，后续回合沿用）。
- R2 thin `deliver_changes` 生成/更新 PR 正文时，按会话的 release_urgency 自动写入 `Release-Urgency: <值>`（最后一段，与 Co-Authored-By 同段规则一致）；模型显式传入的值优先但不得把 immediate 降成空；default 不写。
- R3 界面：会话设置里显示/可改「发版紧急度」。
- R4 测试（先失败）：dispatch 带 immediate → 交付后 PR 正文含 trailer；不带 → 无；hold → 写 hold；模型漏写时仍自动补上。
## 约束
- 授权交付（ceiling: merged），本 PR 正文带 `Release-Urgency: immediate`。不要超出规格自加范围。

## Implementation Notes
- 旧会话默认 default；派单省略字段时保留已有值。派单 hold 是用户决定，不放宽模型自行 hold 的门禁。
- Applicable Harnesses：Spec、Compatibility、Release、Observation、Viewport、AI Collaboration。
- R1/R4：协议、派单持久化和重读；R2/R4：thin create/edit trailer；R3：组件和真实 headless 明暗布局。
- AI Collaboration：context scope 为 dispatch/会话/thin 交付；assumptions 为不新增自动发版行为；review point 为持久化时序、覆盖优先级、hold 来源；validation result 在 PR 记录。
