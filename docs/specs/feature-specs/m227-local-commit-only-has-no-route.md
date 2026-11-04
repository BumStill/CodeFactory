# M227：「只在本地提交、不推送」的请求无路可走（与 M130 两级权限决定矛盾）

## 现场（11-04 05:55，v1.91.75，GPT 模型，信任模式，delivery_authorized=false）
- 用户消息：在测试仓库里追加一行，并**在本地提交**（不推送、不建 PR）。
- 模型的做法依次是：
  1. 用 edit_file 改文件，这一步没问题。
  2. 调用 `deliver_changes`（ceiling: off），触发 `delivery.authorization` 审批「向远端推送并创建 PR / MR 需要本会话的交付授权」，用户拒绝。
  3. 再次明确要求它用 bash 执行 `git add -A && git commit -F - <<MSG`，模型拒绝，原话是：「当前上级规则明确禁止通过 bash 手动提交 Git，也要求文件写入使用专用文件工具。权限门禁验收的用途不能解除这些限制。」
- 最终工作区里还留着未提交的修改，用户要的本地提交没做成。
- 10-20 用户拍板过（M130）：本地 commit 随写权限放行，只有远端动作才看交付授权。现在给模型的规则（系统提示或工具说明）把「提交」整体导向 deliver_changes，而 deliver_changes 即使 ceiling 是 off 也要求交付授权，所以「只要本地提交」这种需求被两头堵死了。
- 同一时期，DeepSeek 会话会直接用 bash 做 git commit，说明不同模型、不同提示路径下的行为不一致。

## 要求
- R1：找到告诉模型「禁止用 bash 手动 git commit」的规则来源（系统提示、工具描述，或者 deliver_changes 的说明），改成和 M130 一致：在本会话工作区里做本地提交（`git add`/`git commit`）是正常的写操作，可以直接用 bash；deliver_changes 只用于需要推送、PR、合并或发版的情况。
- R2：deliver_changes 在 `ceiling: off`（只本地）时不应该要求远端交付授权。要么在工作区本地提交后直接返回，要么返回明确的「请直接用 bash 本地提交」指引。不能弹出「向远端推送并创建 PR」这种和实际动作不符的审批。
- R3：测试：未授权交付的信任会话收到「改文件并本地提交」→ 产生本地提交，不弹审批，不推送；同样的会话收到「推送并开 PR」→ 仍然走交付授权。GPT 和 DeepSeek 两套提示都要覆盖（至少对提示文本做断言）。

## 验收
- 真机：信任模式、未授权交付的会话里，原样发送本次的消息，最后得到本地提交，全程没有审批。

Release-Urgency: immediate

## Implementation Notes
- 选择 R2 允许的无副作用指引路线：显式 ceiling=off 不进入远端授权或交付状态机，提示使用 bash 本地提交。
- Harness：Spec、Compatibility（GPT/DeepSeek 共用提示）、Payload（shell/Git）、Observation（审批和 Git 字段断言）、Release（仅合并，真机验收由编排方发版后完成）、AI Collaboration。
- 测试矩阵：R1 所有 AgentMode 提示与工具说明；R2 实际工具 off 返回值及无远端副作用；R3 未授权信任会话 off 不审批、本地提交真实 Git HEAD 变化且远端不变、远端请求仍审批。
- 根因范围：agent/mod.rs 两处交付提示、tools/delivery.rs 工具说明与 off 路线、permission_gateway.rs 远端动作分类。任务模板要求单执行流，不调用 delegate_tasks。
