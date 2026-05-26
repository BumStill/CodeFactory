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
- Current phase: development
- Current checkpoint: knowledge source refs UI slice implemented on branch `codex/knowledge-source-refs`
- Next owner: development
- Updated at: 2026-05-26

## Completed Items
- 已分析当前 CodeFactory 架构：任务执行、subagent、MCP tools、项目 memory、evidence pack、Git remote/auto PR 已有基础。
- 已确定首期主路径：个人知识库检索辅助 PPT 生成、PowerPoint 插件调用本地服务、多工具 Git 交付。
- 已建立 Req ID、Primary User Path、Applicable Harnesses、测试矩阵和证据包要求。
- 已记录外部最佳实践基线：Office Web Add-in、Office.js、MCP、RAG、GitHub flow、PptxGenJS、Unstructured。
- 已完成安全前置切片：API key 迁移到 OS credential store，Git remote settings 改为只持久化 `token_ref`，新增旧 Git token 迁移和 UI DTO 脱敏。
- 已收紧 Git remote 运行时：远端操作按需通过 `token_ref` 解析 token；旧 inline token 未迁移成功时不再作为 fallback 使用。
- 已完成知识库后端 MVP：新增知识库 SQLite schema、注册/列表/扫描/检索 Tauri commands、`kb_search`/`kb_get_chunk` Agent tools、DOCX/PPTX/PDF 极简文本抽取、检索审计事件。
- 已完成知识库前端 connector MVP：Workspace 右栏显示个人知识库状态、添加入口、扫描按钮和扫描摘要；任务创建弹窗显示本次可见知识库数量，避免静默注入。
- 已完成任务执行侧知识库上下文闭环：`create_task_tree` payload 持久化 `TaskConnectorContext`，TaskCreator/TaskRow 展示任务知识库范围，scheduler brief 显式暴露 `kb_search`/`kb_get_chunk`，知识库工具按任务 scope 限制检索并记录 `session_id`/`task_id`，evidence pack 输出 `knowledge_refs.json`。
- 已完成知识库来源可见化切片：`kb_search` 工具结果展开时展示本地文件名、页码/slide、chunk、片段；Evidence Pack 增加 `Sources` 页签展示 `knowledge_refs.json` 中的 query、latency 和来源文件。
- 已收窄 MVP 边界：首期目标是可靠参考本地文件做事、办公和开发，不先做团队知识库、云同步、多租户权限、后台索引平台或多向量数据库配置矩阵。

## Remaining Items
- 规划阶段：
  - 确认首期文档格式、embedding provider、PPT renderer、Office 插件分发方式。
  - 拆分实施规格：Knowledge MVP、PPT Renderer、Office Bridge、Git Delivery Orchestrator、Assistant Connectors UI。
- 开发阶段：
  - 补齐 secret migration 的集成测试：旧 `keys.json` 迁移、backup/import 脱敏、GitHub/GitLab header mock、OS credential store 失败路径。
  - 扩展知识库管理 UI：失败文件列表、引用来源卡片、禁用/启用开关和索引详情抽屉。
  - 扩展真实执行流聚合视图：在任务级时间线中汇总 `kb_search` / `knowledge_refs.json` 的来源统计，避免用户只在单个工具卡或 evidence pack 中查看。
  - 升级检索：FTS/向量检索、metadata filter、token budget、真实 PDF 解析质量和 Office XML 结构保留。
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
  - `/Users/leo/.cargo/bin/cargo test git_remote_ --lib`：通过，4 个 Git remote secret migration/脱敏测试。
  - `/Users/leo/.cargo/bin/cargo check`：通过，存在既有 warning。
  - `pnpm build`：通过。
  - `pnpm test`：通过，7 files / 28 tests。
  - `python3 tools/governance/validate_repo_governance_baseline.py`：通过。
  - `git diff --check`：通过。
  - `/Users/leo/.cargo/bin/cargo test knowledge --lib`：通过，覆盖 DOCX/PPTX/PDF 扫描、损坏文件不中断、`kb_search` tool 数据库检索和来源 JSON。
  - `/Users/leo/.cargo/bin/cargo test ensure_schema_creates_satellite_tables_on_fresh_db --lib`：通过，覆盖知识库表的幂等 schema 创建。
  - `/Users/leo/.cargo/bin/cargo check`：通过，存在既有 warning。
  - `pnpm test -- src/stores/knowledge.test.ts src/pages/Workspace/TaskCreator.test.tsx`：通过，8 files / 33 tests；覆盖知识库 store command 参数、扫描摘要、失败状态、Workspace connector 可见性和任务弹窗知识库摘要。
  - Browser viewport check：本地 Vite mock preview，1366x768 与 900x768；确认 Workspace 右栏知识库摘要、扫描结果、任务弹窗 `知识库 1` 可见，`scrollWidth == innerWidth`，无水平滚动。
  - `pnpm test -- src/stores/tasks.test.ts src/pages/Workspace/TaskCreator.test.tsx`：先失败后通过；覆盖 `create_task_tree` payload 携带知识库 context、TaskCreator 审核页展示任务上下文。
  - `pnpm test`：通过，9 files / 34 tests。
  - `pnpm build`：通过，存在既有 large chunk warning。
  - `/Users/leo/.cargo/bin/cargo test ensure_schema_creates_satellite_tables_on_fresh_db --lib`：通过，覆盖 `task_runs.task_context_json` 幂等 schema 创建。
  - `/Users/leo/.cargo/bin/cargo test kb_search_uses_attached_database_and_returns_source_json --lib`：先编译失败后通过；覆盖 `kb_search` 从 task scope 默认限制 library、写入 `retrieval_events.session_id/task_id/filters_json`。
  - `/Users/leo/.cargo/bin/cargo check`：通过，存在既有 warning。
  - `pnpm test -- src/components/ToolCallCard.knowledge.test.tsx src/components/EvidenceViewer.knowledge.test.tsx`：先失败后通过；覆盖 `kb_search` 展开来源卡片和 Evidence Pack `Sources` 页签。
  - Browser viewport check：隔离 worktree 本地 Vite visual harness（端口 5190），1366x768 与 390x768；确认 `kb_search` 来源卡片和 Evidence Pack `Sources` 页签中 `roadmap.pptx`、`slide 4`、`chunk-1`、`42ms` 可见，`scrollWidth == innerWidth`，无水平滚动；临时 harness 已删除。
  - `pnpm test`：通过，13 files / 49 tests。
  - `pnpm build`：通过，存在既有 large chunk warning。
  - `python3 tools/governance/validate_repo_governance_baseline.py`：通过。
  - `git diff --check`：通过。
- Release evidence:
  - not live：尚未实现 PowerPoint 插件、安装包集成或真实 Office 主路径。
- Blocking evidence:
  - `/Users/leo/.cargo/bin/cargo test --lib`：49/50 通过；既有 `tools::bash::tests::successful_command_output_includes_shell_audit_metadata` 在 macOS 上因测试固定调用 `powershell` 失败，未混入本切片修复。

## AI Collaboration
- context scope: CodeFactory repo docs、当前任务执行/MCP/memory/Git remote 代码、secret store、Knowledge backend、Office Add-in/RAG/MCP/GitHub flow 外部设计基线。
- assumptions: 首期以本地个人知识库为边界；知识库 MVP 先用本地 SQLite 表和极简 DOCX/PPTX/PDF 文本抽取，FTS/向量和高保真解析后续升级；PowerPoint 插件使用 Office Web Add-in；PPT 生成优先用 Node/PptxGenJS sidecar；Git 自动 merge 默认关闭。
- review point: planning sub-agent 和 QA sub-agent 审阅规格；development/QA sub-agent 审阅 secret storage 前置切片风险；knowledge explorer sub-agent 审阅后端落点、schema 和测试风险；Workspace connector UI explorer 审阅 UI 接入点和 viewport 风险；knowledge task-context explorer 审阅任务 payload、ExecCtx、scheduler brief、evidence 和 viewport 风险；knowledge source refs explorer 审阅最小 UI 落点，建议只改 `ToolCallCard` 和 `EvidenceViewer`。
- validation result: Git remote secret migration targeted tests、knowledge backend targeted tests、knowledge connector UI/store tests、knowledge task context/evidence targeted tests、knowledge source refs UI targeted tests、cargo check、frontend build/test、governance baseline 和 diff check 已通过；完整 Rust lib suite 存在既有跨平台 bash 测试失败；PowerPoint/知识库完整 UI 主路径仍是 `not live`。

## Stop Boundary
- Do not stop after local-only validation.
- Do not stop after deploy output without live verification.
- Stop only when done or explicitly blocked with evidence.
