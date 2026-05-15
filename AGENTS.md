# CodeFactory 仓库协作规则（AGENTS）

## 入口顺序
- 日常任务先读 `docs/repo-quick-profile.md`、当前任务说明和 quick gate 结果。
- 只有 quick gate 标记 release、compatibility、observation、payload、viewport 或 governance-change 风险时，才加载 `docs/repo-governance-profile.md` 和全局治理参考。
- 本仓库采用 `codex-delivery-governance`，全局标准入口为 `multi-project-harness-standard.md`。

## 全局治理源
- `codex-delivery-governance`: `C:/Users/yourz/.codex/skills/codex-delivery-governance/SKILL.md`
- `multi-project-harness-standard.md`: `C:/Users/yourz/.codex/skills/codex-delivery-governance/references/multi-project-harness-standard.md`
- `governance-change-control.md`: `C:/Users/yourz/.codex/skills/codex-delivery-governance/references/governance-change-control.md`
- 仓库本地只能适配 surface、主路径、命令、平台、证据和产品约束；不得在本仓库重定义全局 Harness 分类、对象模型、选择规则、最低证据或 validator contract。

## 文档语言
- 新建或大幅修改的项目文档、规格、计划、长任务记录、证据包和治理记录默认使用中文。
- 代码标识、命令、API 字段、日志、错误信息和第三方接口名保留英文。

## 角色门禁
- 规划 / 系统工程角色：整理规格、Requirements Traceability、Primary User Path、测试矩阵和 Applicable Harnesses。
- 开发角色：只按已批准规格实现，按 Req ID 记录变更，保持最小 patch。
- QA / 验收角色：独立验证真实用户路径，有权拒绝仅靠结构检查、HTTP 200、非空数组或 mock 成功的完成声明。
- 发布 / 运维角色：仅在 release-facing 任务中启用，负责部署、配置、回滚和 live verification。
- 非平凡产品、代码、发布、治理变更，在工具环境允许且用户未关闭时默认启动真实后台角色 sub-agent。允许跳过的情况：只读或极小变更、工具不可用、用户要求单执行流、安全审批未完成、任务无法安全拆分。跳过时最终输出必须写明原因。

## Harness 范围
- 所有实现或治理任务至少启用 Spec Harness。
- release-facing 或生产可见任务同时启用 Compatibility Harness、Release Harness、Observation Harness。
- 上传、文件、音视频、请求体、网关限制、对象存储相关任务追加 Payload Harness。
- 移动端、首屏、固定操作区、溢出、动画或响应式布局相关任务追加 Viewport Harness。
- AI 生成代码、多角色协作、治理修改或关键假设不清时追加 AI Collaboration Harness。

## CodeFactory 主路径
- Primary User Path: 用户打开 CodeFactory，选择项目工作目录和模型，输入编程任务，模型读取项目、提出或执行受控工具调用，用户审批高风险操作，系统展示 diff、命令输出、测试结果和会话记录。
- release-facing 缺陷默认必须完成本地修复、相关测试、发布交接或部署、真实主路径验证；如果不能发布，最终输出必须明确写 `not live`、阻塞原因和下一步命令或人工动作。

## 证据最低线
- 不得单独把 UI 可见、HTTP `200`、mock 通过、非空数组、行数正确、deploy 命令成功或本地测试通过当成完成证据。
- 工具、文件、命令、OpenRouter、SQLite、安装包和发布路径的变更必须记录实际处理 route、字段级断言或主路径证据。
- AI Collaboration 最小记录：context scope、assumptions、review point、validation result。

## 验证与测试
- 行为或代码修改前必须先写独立测试或可执行验收，并先看到失败。
- 修改后运行相关测试；若存在 lint、typecheck、build，应优先补齐并执行。
- 未运行验证命令时，不得声称完成，只能说明已修改并待验证。
- 本仓库治理基线验证命令：`python tools/governance/validate_repo_governance_baseline.py`。

## 规格与长任务
- 长期存在的业务能力、架构约束、主路径、兼容语义和验收基线必须写入 `docs/specs/`，不能只存在于一次性计划。
- 长任务使用 `docs/long-tasks/` 记录；`long tasks` 只能在完成或有证据阻塞时停止。
- `docs/planning-agent.md`、`docs/development-agent.md`、`docs/qa-acceptance-agent.md`、`docs/release-ops-agent.md` 是角色协作入口。

## governed CI/CD
- `.github/workflows/governance-baseline.yml` 负责在 PR/push 上检查治理基线文件。
- `.github/workflows/governed-delivery.yml` 是手动触发的治理交付入口。
- 改 CI、发布配置、生产配置、schema、依赖或删除数据前必须先说明风险。

Repository: `CodeFactory`
