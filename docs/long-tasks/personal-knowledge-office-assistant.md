# Personal Knowledge and Office Assistant 长任务记录

## Basics
- Task ID: `LT-PERSONAL-KNOWLEDGE-OFFICE-ASSISTANT`
- Title: 个人知识库、PowerPoint 插件和通用助手化
- Feature spec: `docs/specs/feature-specs/personal-knowledge-office-assistant.md`
- Related Req IDs: `CF-KB-R1`、`CF-KB-R2`、`CF-KB-R3`、`CF-KB-R4`、`CF-PPT-R1`、`CF-PPT-R2`、`CF-PPT-R3`、`CF-GIT-R1`、`CF-GIT-R2`、`CF-GA-R1`

## Completion Standard
- Done means:
  - 用户可注册本地知识库文件夹，成功索引样本 PPTX、DOCX、PDF，并在任务执行中通过可审计工具检索引用。
  - 用户可在 PowerPoint 中 sideload CodeFactory 插件，完成本地服务配对，生成 PPTX 或插入 slides。
  - CodeFactory 可对开发任务执行 Git preflight、隔离分支或 worktree、验证、commit/push/PR，并在 required checks 通过后合并或交接。
  - 相关规格、测试矩阵、证据包和 release blocker 都已更新。
- Blocked means:
  - Office Add-in sideload、HTTPS/证书、PowerPoint API、文档解析、embedding provider、Git remote/checks 中任一关键路径无法验证。
  - 必须记录 blocker、失败证据、下一步命令或人工动作。

## Current State
- Current phase: planning
- Current checkpoint: feature spec and long-task record created
- Next owner: planning
- Updated at: 2026-05-26

## Completed Items
- 已分析当前 CodeFactory 架构：任务执行、subagent、MCP tools、项目 memory、evidence pack、Git remote/auto PR 已有基础。
- 已确定首期主路径：个人知识库检索辅助 PPT 生成、PowerPoint 插件调用本地服务、多工具 Git 交付。
- 已建立 Req ID、Primary User Path、Applicable Harnesses、测试矩阵和证据包要求。
- 已记录外部最佳实践基线：Office Web Add-in、Office.js、MCP、RAG、GitHub flow、PptxGenJS、Unstructured。

## Remaining Items
- 规划阶段：
  - 确认首期文档格式、embedding provider、PPT renderer、Office 插件分发方式。
  - 拆分实施规格：Knowledge MVP、PPT Renderer、Office Bridge、Git Delivery Orchestrator、Assistant Connectors UI。
- 开发阶段：
  - 先迁移 API/Git/Office token 到 OS credential store 或等价安全 secret store，settings 和文件型 `keys.json` 不再承载明文 token。
  - 新增 SQLite schema 和 settings 默认值迁移。
  - 实现知识库注册、扫描、解析、FTS/向量检索和知识库工具。
  - 实现 `pptx_plan.json` schema、renderer、PNG QA 和 artifact 管理。
  - 实现本地 HTTPS bridge service、pairing token、PowerPoint add-in manifest/task pane。
  - 实现 Git preflight、branch/worktree、verification gate、finalize/PR/checks。
- QA 阶段：
  - 建立样本知识库 fixture 和损坏/大文件/权限失败用例。
  - 建立 Office Add-in sideload 手工验收和可自动化的 local service 测试。
  - 建立 Git dirty tree、behind base、required checks failed、stage ownership 等用例。
- Release 阶段：
  - 将 add-in manifest、local service、证书/loopback 指引纳入安装包或安装向导。
  - 完成真实 PowerPoint 主路径 smoke 前不得宣称 live。

## Blockers
- None at planning stage.

## Evidence
- Local evidence:
  - `docs/specs/feature-specs/personal-knowledge-office-assistant.md`
  - `docs/long-tasks/personal-knowledge-office-assistant.md`
- Release evidence:
  - not live：尚未实现 PowerPoint 插件、安装包集成或真实 Office 主路径。
- Blocking evidence:
  - None.

## AI Collaboration
- context scope: CodeFactory repo docs、当前任务执行/MCP/memory/Git remote 代码、Office Add-in/RAG/MCP/GitHub flow 外部设计基线。
- assumptions: 首期以本地个人知识库为边界；PowerPoint 插件使用 Office Web Add-in；PPT 生成优先用 Node/PptxGenJS sidecar；Git 自动 merge 默认关闭。
- review point: planning sub-agent 和 QA sub-agent 审阅规格；实现前需要确认首期切片。
- validation result: governance baseline 和 long-task validator 已通过；产品主路径仍是 `not live`。

## Stop Boundary
- Do not stop after local-only validation.
- Do not stop after deploy output without live verification.
- Stop only when done or explicitly blocked with evidence.
