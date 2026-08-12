# Objective Recovery Control Plane：UX 设计

## 设计目标

用户始终能回答三件事：系统是否仍持有目标、最近发生了什么真实进展、何时才确实需要我。技术细节默认折叠，控制语义只能来自 typed objective state，不能从错误字符串或 tool status 猜测。

## 状态层级

| State | 标题 | 信息 | CTA |
| --- | --- | --- | --- |
| `active/recovering/waiting_system` | 正在继续处理 | owner、阶段、最近真实进展、下次安全观察、累计时间 | 无；可展开详情或显式取消 |
| `waiting_core_input` | 需要一项核心输入 | 已尝试路径、合并后的最小缺项、输入后的自动续接点 | 唯一的输入/授权动作 |
| `waiting_business_decision` | 需要业务决定 | 互斥选项、推荐项、各自业务影响、系统不能代选原因 | 一组互斥选择 |
| `reconciling_side_effect` | 正在核对已发生操作 | receipt/远端状态未知、禁止盲重放说明 | 无 |
| `completed` | 已完成 | evidence ref、达到的验收层级、delivery ladder | 查看证据 |
| `cancelled` | 已停止 | 谁、何时、取消的 action/objective、已保留副作用 | 可创建新目标；不自动恢复 |
| `legacy_orphan` | 历史任务无法安全恢复 | 缺失的身份字段和零副作用保证 | 只读查看；不伪造恢复 |

## 会话与恢复卡

- objective 卡与原 root turn 绑定，不新增“重新开始”消息。
- stream/turn 结束后，只要 objective 仍 system-owned，卡继续显示；刷新和 App 重启后从 DB hydration 恢复。
- card 顶部只展示一句当前状态；owner、failure code、attempt、最近进展和 next observation 放在可展开详情。
- heartbeat 不更新“最近进展”；只有 receipt、验证、head/revision 或 state transition 才更新。
- 同一 objective 的 provider/task/delivery 阶段在一张卡内演进，不堆叠多个警告。

## 必要输入

- core input 卡一次列出同 objective 的全部缺项，禁止碎片化弹窗。
- 说明“完成此输入后系统将自动从 X 继续”，不出现“然后点击重试/重新发送”。
- OAuth、permission、browser pairing/2FA 完成事件到达后，卡立即变为“已收到，正在续接”。
- 输入仍未满足时维持等待，不启动相同副作用；请求计数保持 1。

## 权限交互

- 普通已授权且可逆动作不弹窗；高风险、权限扩大、不可逆动作保留 ask/hard deny。
- 60 秒倒计时只表示本次 prompt channel 生命周期，不等于目标失败。timeout 后卡变为“授权通道已暂停，系统将保持任务并等待安全续接”。
- 显式拒绝清楚说明受拒 action signature，不允许模型换工具绕过。

## Task Workspace

- 删除“重试失败步骤”“已修复，重试”“回到对话处理”和伪 user message 注入。
- 技术失败任务行显示 `正在诊断/正在修复/等待退避/正在验证`；必要输入才显示对应设置或授权动作。
- Resume summary 进入正式 `TasksColumn`，而不是只存在旧 TaskDashboard/验收壳。
- 多个任务共享同一 core input 时聚合为 session/objective 级请求，任务行引用该请求。

## Auth 与 Provider

- auth expired 显示一次重新验证入口；验证成功后文案为“已重新连接，正在从安全检查点继续”。
- rate limit/5xx/endpoint transient 显示自动退避或切换 route，无人工重试按钮。
- 全 route 技术性耗尽显示 platform incident owner；只有确认缺失外部凭据/额度且无替代 route 才显示 core input。
- 已有可见输出或副作用未知时显示“正在核对”，不宣称自动重放。

## Delivery 与 Release

- 保留现有 PR、CI、merge、release、live 五层证据。
- branch policy、merge queue、version PR、workflow failure 归入同一 objective 的 release stage，不生成“重新发布”CTA。
- requested acceptance 永远可见；低层级完成不折叠为最终完成。

## 文案禁区

对 system-owned 技术状态禁止出现：

- “回复继续”“继续执行”“稍后重试”；
- “已修复，重试”“重试失败步骤”；
- “重新发送需要继续的内容”；
- “回到对话处理”及任何伪造 user message；
- “请人工核对/切分支/选择 PR”作为恢复触发器。

“重试”可出现在只读技术详情或用户明确发起的新目标中，但不得成为当前 objective 的必要 CTA。

## Viewport 与可访问性

- 1366×768：卡不遮挡 composer、delivery drawer 和 task column。
- 800×600：状态摘要单行可换行，详情和 CTA 不溢出。
- 390×844：核心输入/业务决定动作纵向排列，最小 44px 触控区域，无横向滚动。
- `role=status` + `aria-live=polite` 用于 system progress；core input/business decision 使用明确 heading/description，不用高频 live announcement。
- 状态不能只靠颜色；图标、标题和文本必须共同表达。
- motion-reduce 下禁用旋转动画，保留静态“系统仍在处理”标识。

## 实地验收

真实 App 至少录制：permission timeout 后自动续接、provider 503 切换、task verification 耗尽进入 remediation、auth 恢复自动续接、App kill/restart、CI/version PR 等待、显式拒绝、必要业务决定。每条同时验证正常路径和防重复副作用边界。
