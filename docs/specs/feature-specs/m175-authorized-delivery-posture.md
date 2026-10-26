# M175（高）：授权交付的派单不得被文字推断降级为只读/规划姿态

## 证据（v1.91.35，10-26 14:34–14:44）
编排方把规格全文 + task-template 内联进 create_and_send 消息（delivery_authorized=true、permission_mode=trusted），3 个会话（712c7b14 M174 / 5aea09e3 M173 / 4fd26823 M170）全部：
- 工具层返回「当前是只读任务」「review-only，仅允许写规划文档」；
- 未分配托管工作区，cwd 为用户主 checkout（/Users/leo/Projects/CodeFactory，main，落后 16 提交），在其中写了 docs/plans/m17x-*.md；
- 以「尚未实施」结束，零交付。
同样规格以短消息「开始实施：按 <规格> 修复…授权交付」派发时正常（T1 零介入）。推断因内联模板里大量「只读/review/规划」字样。
## 原则
用户约定：只许猜姿态，不许猜写权限；推断出的只读不可恢复，故必须保守。delivery_authorized=true 是明确的结构化授权。
## 需求
- R1 dispatch/会话带 delivery_authorized=true（或界面「允许交付」开）时，姿态固定为实施，文字推断不得降级为只读/规划；必须分配托管工作区，绝不在用户主 checkout 写文件。
- R2 姿态推断只看用户本人指令的意图部分，忽略引用/粘贴的规格、模板、日志中的词（或对长消息仅取首段指令）。
- R3 若推断为只读但消息含实施授权，按实施处理并在 PR/对话里记录推断冲突。
- R4 测试（先失败）：上述长消息 + delivery_authorized=true → 实施姿态 + 托管工作区；无授权的「只读审查一下」→ 只读；不在主 checkout 写文件。
## 约束
- task-template；合成 fixture；授权交付（ceiling: merged）。

Release-Urgency: immediate

## Implementation Notes
- Req ID 与测试映射、Harness 证据见本 PR。结构化 review-only 子任务仍是明确权限边界，不会被会话交付开关覆盖。
- 合成派单只复现消息形状，不使用证据中的真实会话 ID、用户消息或路径。
- 真机验收由编排方发版安装后完成：用内联规格与模板发起 create_and_send，确认实施姿态、托管 cwd、写文件和交付可用；关闭授权后只读审查仍拒绝产品写入。
