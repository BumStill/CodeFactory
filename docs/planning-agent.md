# Planning Agent

## 目标
- 在开发前把需求转成可执行规格，明确 Requirements Traceability、Primary User Path、Applicable Harnesses 和验收矩阵。
- 维护 `docs/specs/` 作为长期规格承载层，不能让一次性计划替代稳定业务规则。

## 必须产出
- Requirements Traceability：每条需求必须有 Req ID、来源、影响 surface、验证方式和责任角色。
- Primary User Path：每个特性只能指定一个主路径，先验证主路径，再验证边界路径。
- Applicable Harnesses：至少包含 Spec Harness；按触发条件追加 Compatibility、Release、Observation、Payload Harness、Viewport Harness、AI Collaboration。
- 测试矩阵：正常路径、失败路径、兼容路径、发布路径和人工验收边界。
- Scenario Traceability：从 `docs/testing/scenario-registry.json` 选择受影响的 Scenario ID；新能力没有匹配 ID 时先登记场景，再批准实现。
- Evidence Pack 要求：说明截图、字段级断言、route selection、build metadata、live verification 的最小证据。

## CodeFactory 默认主路径
- P1: 用户打开 CodeFactory，选择项目 cwd 和模型，输入编程任务，模型读取项目并提出工具调用，高风险操作经用户确认，系统展示 diff、命令输出、测试结果和会话记录。

## 规划拒绝条件
- 没有 Req ID。
- 没有 Primary User Path。
- 产品变更没有 Scenario ID，或只写测试文件名而没有说明需要达到的证据等级。
- 只写实现步骤，不写验证和证据。
- 把 UI 出现、HTTP 200、mock 成功或非空数组当作完成。
- 对 OpenRouter、文件写入、命令执行、SQLite、发布包等高风险路径未列出字段级断言或 route selection。

## Long Task
- 跨多阶段、跨角色或 release-facing 的任务必须创建 `docs/long-tasks/` 记录。
- 长任务记录必须明确完成标准、阻塞标准、当前阶段、下一责任角色和证据链接。
