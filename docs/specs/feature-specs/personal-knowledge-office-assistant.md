# Personal Knowledge and Office Assistant 规格

## 范围
- 本规格定义 CodeFactory 从 AI 编程 Agent 演进为本地通用任务助手的下一阶段能力。
- 首期聚焦三条能力线：
  - 个人知识库：注册本地资料文件夹，解析 `pptx`、`docx`、`pdf` 等文档，在任务执行时按需检索、引用和复用。
  - PowerPoint 插件：在 PowerPoint 内通过 Office Web Add-in 调用 CodeFactory 本地助手服务，生成或插入 PPT 内容。
  - 多工具 Git 交付：每个开发任务在执行前同步分支，使用隔离分支或 worktree，验证后提交、推送、创建 PR，并按检查结果合并或交接。
- 不在首期承诺云端同步、团队知识库、多用户权限系统、AppSource 上架、自动发布到生产安装包。

## 外部最佳实践基线
- Office 插件使用 Microsoft Office Web Add-in 模型：PowerPoint task pane、ribbon command、Office.js API、本地开发 HTTPS、manifest sideload。
- PowerPoint 内容插入首期使用 `PowerPoint.Presentation.insertSlidesFromBase64`，默认继承目标文档主题，提供保留源格式选项。
- 知识库遵循 RAG 设计：文档解析、结构化 chunk、metadata 过滤、关键词检索、向量检索、来源引用、权限边界、检索事件审计。
- 扩展协议优先兼容 MCP 的 Tools、Resources、Prompts 思路：工具可见、参数可审计、失败可回放，不把隐式能力塞进不可见 prompt。
- Git 交付遵循 GitHub flow：短生命周期分支、PR、required checks、review/approval、合并后清理分支；本地 dirty tree 必须先被显式处理。

参考入口：
- Microsoft Office Add-ins: `https://learn.microsoft.com/en-us/office/dev/add-ins/`
- PowerPoint JavaScript API: `https://learn.microsoft.com/javascript/api/powerpoint/`
- MCP specification: `https://modelcontextprotocol.io/specification/`
- GitHub flow: `https://docs.github.com/en/get-started/using-github/github-flow`
- GitHub protected branches: `https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches`
- PptxGenJS: `https://github.com/gitbrent/PptxGenJS`
- Unstructured document partitioning: `https://docs.unstructured.io/open-source/core-functionality/partitioning`

## Requirements Traceability
| Req ID | User request | Normalized requirement | Surfaces | Validation method | Owner |
| --- | --- | --- | --- | --- | --- |
| CF-KB-R1 | 有个文件夹，里面有大量 PPT 和 Word 等文档，需要分析或写新 PPT 时完全参考复用 | 用户可注册一个或多个本地知识库文件夹，系统增量扫描并解析 `pptx`、`docx`、`pdf`，保留文档结构、页码/slide、标题、正文、表格、图片引用和更新时间 | desktop-ui + tauri-backend + sqlite-store + file-indexer | 使用样本知识库断言文件发现、hash 去重、更新时间检测、解析 route、chunk metadata 和失败文件记录 | planning + development |
| CF-KB-R2 | 任务执行部分增强到能使用个人知识库 | Agent 执行任务时可通过工具检索知识库，返回有来源的上下文片段，并在 UI 和审计记录中展示引用来源 | agent-loop + tool-runtime + desktop-ui + sqlite-store | `kb_search` / `kb_get_chunk` 字段级测试，模型工具调用记录，UI 来源卡片截图，SQLite retrieval event 抽样 | development + qa |
| CF-KB-R3 | 写新 PPT 时思路能参考复用 | PPT 生成链路必须先产出带来源引用的 outline 和 `pptx_plan.json`，再生成 PPTX；每页记录参考来源、意图、版式和素材 | ppt-renderer + knowledge-service + desktop-ui | 代表性任务生成 outline、plan、PPTX 和 PNG 渲染；断言 slide 数、标题、引用、版式 token、无空白页 | qa |
| CF-KB-R4 | 参考业界、GitHub 最佳实践 | 规格、实现计划和证据包必须记录外部设计基线，并把兼容、安全、检索质量、Git workflow 作为验收项 | docs + evidence-pack | 规格审查确认外部基线、风险边界和测试矩阵存在 | planning |
| CF-PPT-R1 | 本地启动工具后能有插件安装到 PowerPoint 里 | CodeFactory 提供 PowerPoint Web Add-in manifest、安装向导和本地 HTTPS bridge service；插件可连接本地助手服务 | office-addin + local-service + desktop-ui | sideload 后 PowerPoint task pane 可见，配对成功，`/health` 和 `/pair` 字段断言 | development + qa |
| CF-PPT-R2 | 插件和本地助手服务互相调用 | 插件通过一次性 pairing code 绑定本地服务，后续用短期 token 调用受限 API；服务只监听 loopback，不接受任意网页未授权调用 | office-addin + local-service + credential-store | CORS/origin、token、过期、拒绝未配对请求、审计字段测试 | qa |
| CF-PPT-R3 | 直接在 PPT 里生成 PPT 文件 | 插件可请求生成完整 PPTX 或插入 slides；生成结果可在 PowerPoint 当前演示文稿中预览和插入 | office-addin + ppt-renderer + local-service | 端到端 sideload：输入任务、选择知识库、生成 PPTX、插入当前 deck、截图/录屏 | qa |
| CF-GIT-R1 | 多个工具里开发，开发前拉取分支，验证后合并提交 | CodeFactory 任务执行前执行 Git preflight：识别 base branch、fetch/pull、dirty tree、分支策略、worktree 策略和权限风险 | git-orchestrator + desktop-ui + tool-runtime | dirty tree、behind base、detached HEAD、非 git repo、远端不可达用例 | development |
| CF-GIT-R2 | 验证后合并提交 | 任务完成后只在验证通过或用户确认降级时允许 stage/commit/push/PR；required checks 通过后才能自动合并，否则创建 draft PR 或交接说明 | git-orchestrator + git-remote + evidence-pack | commit message、PR body、check status、merge decision、blocked handoff 字段断言 | qa + release |
| CF-GA-R1 | 往通用助手类演进 | CodeFactory 主界面把任务类型、知识库、Office、Git、MCP、Skills 作为可见 connector 和可控上下文，不再只围绕代码聊天 | desktop-ui + settings + agent-loop | 主路径截图，任务上下文摘要，启用/禁用 connector 的 tool list 和权限变化测试 | planning + qa |

## Primary User Path
P-KB-PPT-1: 用户打开 CodeFactory，注册一个本地个人知识库文件夹。系统扫描并解析其中的 PPT、Word、PDF 文档，展示索引状态、失败文件和可检索来源。用户打开 PowerPoint 并加载 CodeFactory 插件，插件通过一次性配对码连接本地 CodeFactory 服务。用户在插件中输入“基于我的历史方案资料生成一份 10 页产品路线图 PPT”，选择目标知识库和输出选项。CodeFactory 创建任务，检索知识库，展示带来源引用的 outline，用户确认后生成 `pptx_plan.json` 和 PPTX，完成渲染 QA。插件把生成的 slides 插入当前演示文稿，并展示引用来源和生成记录。

P-GIT-DELIVERY-1: 用户在 CodeFactory 内启动一个开发任务。系统先执行 Git preflight，显示当前分支、base branch、dirty tree、远端状态和建议策略。用户确认后系统 fetch/pull base，创建 `codex/<task-slug>` 分支或 worktree。Agent 按 Req ID 执行、运行验证、生成 evidence pack。验证通过后系统 stage、commit、push、创建 PR，读取 required checks；checks 通过且用户授权时合并，否则保留 PR 并输出 blocker 和下一步动作。

## Applicable Harnesses
- Spec Harness: 本规格、Req ID、Primary User Path、测试矩阵和证据包要求必须存在。
- Payload Harness: 文档解析结果、PPTX、DOCX、PDF、图片、嵌入向量、检索 chunk、PPTX artifact、PowerPoint 插件请求体都属于 payload。
- Compatibility Harness: 新增 SQLite schema、settings 结构、旧 session、旧 MCP 配置、旧 permission 策略、旧 memory 文件都需要迁移或兼容证据。
- AI Collaboration Harness: 知识库检索、PPT 规划、multi-agent 执行、自动 Git 交付都必须记录 context scope、assumptions、review point、validation result。
- Viewport Harness: 新的知识库管理页、任务上下文面板、引用卡片、PowerPoint task pane 和权限弹窗必须覆盖常用桌面和窄宽度。
- Observation Harness: 索引耗时、解析失败、检索命中、引用使用、生成耗时、PPT 插入失败、Git preflight/PR/checks 状态必须记录。
- Release Harness: PowerPoint 插件安装向导、本地 HTTPS 服务、Windows 证书/loopback、安装包和更新通道属于 release-facing 边界；未发布前最终声明必须写 `not live`。

## 架构设计
### 组件
| Component | Responsibility | First implementation boundary |
| --- | --- | --- |
| Knowledge Library Manager | 管理知识库文件夹、扫描策略、索引状态、失败文件和用户可见开关 | Tauri command + SQLite + UI 列表 |
| Document Ingestion Worker | 增量解析 `pptx`、`docx`、`pdf`，抽取文本、结构、表格、图片引用和 slide/page metadata | 本地异步 worker，首期只支持现代 Office XML 和 PDF |
| Retrieval Engine | FTS、metadata filter、向量检索、rerank、引用拼装和 token budget 控制 | SQLite FTS5 + embedding provider 抽象；向量可先本地表存储，后续替换 vector extension |
| Knowledge Tools | 给 Agent 暴露 `kb_search`、`kb_get_chunk`、`kb_get_document_outline`、`kb_get_deck_profile`、`kb_retrieve_slide_examples` | tool-runtime + permission policy |
| PPT Planning Pipeline | 将用户意图和检索结果转为 outline、引用矩阵、`pptx_plan.json` | Agent task + schema validator |
| PPT Renderer | 将 `pptx_plan.json` 渲染为 PPTX，并输出 PNG QA 证据 | Node sidecar + PptxGenJS；后续再评估 Rust/OOXML |
| Office Bridge Service | 本地 HTTPS loopback 服务，处理 health、pair、task、artifact、insert 状态 | Tauri sidecar/local service |
| PowerPoint Add-in | PowerPoint task pane + command，展示配对、任务输入、生成状态、插入按钮和来源 | Office.js + manifest sideload |
| Git Delivery Orchestrator | Git preflight、branch/worktree、verification gate、commit/push/PR/checks/merge | Rust backend + existing git/git_remote commands |

### 数据对象
| Object | Key fields |
| --- | --- |
| `knowledge_libraries` | `id`, `name`, `root_path`, `enabled`, `created_at`, `last_scan_at`, `scan_status`, `include_globs`, `exclude_globs` |
| `knowledge_documents` | `id`, `library_id`, `path`, `kind`, `hash`, `mtime`, `size`, `title`, `author`, `status`, `error` |
| `knowledge_chunks` | `id`, `document_id`, `chunk_index`, `content_type`, `text`, `page`, `slide`, `heading`, `token_estimate`, `metadata_json` |
| `knowledge_embeddings` | `chunk_id`, `model`, `dim`, `vector_blob`, `created_at` |
| `knowledge_assets` | `id`, `document_id`, `asset_type`, `page_or_slide`, `path_or_blob_ref`, `metadata_json` |
| `deck_profiles` | `document_id`, `theme_json`, `fonts_json`, `colors_json`, `layout_summary_json`, `thumbnail_refs_json` |
| `retrieval_events` | `id`, `session_id`, `task_id`, `query`, `filters_json`, `result_refs_json`, `created_at`, `latency_ms` |
| `office_pairings` | `id`, `app`, `device_label`, `token_ref`, `expires_at`, `created_at`, `last_used_at` |
| `office_requests` | `id`, `session_id`, `request_type`, `ppt_context_json`, `status`, `artifact_path`, `evidence_path`, `error` |
| `delivery_runs` | `id`, `session_id`, `base_branch`, `working_branch`, `worktree_path`, `preflight_json`, `verification_json`, `pr_url`, `status` |

API Key、Git token、Office pairing token 必须进 OS credential store 或等价 secret store，不得进入 settings 明文、文件型明文 secret store、日志、导出文件或测试快照。当前文件型 `keys.json` 只能作为过渡状态，扩展 Office pairing、Git 自动交付或知识库外发 embedding 前必须升级。

### Agent 工具
| Tool | Purpose | Permission default | Required audit fields |
| --- | --- | --- | --- |
| `kb_search` | 按 query、library、document kind、metadata filter 返回引用候选 | allow | `query`, `filters`, `top_k`, `result_count`, `chunk_ids`, `latency_ms` |
| `kb_get_chunk` | 读取具体 chunk 的完整文本和 metadata | allow | `chunk_id`, `document_id`, `page_or_slide`, `bytes_returned` |
| `kb_get_document_outline` | 返回 Word/PDF 章节或 PPT slide outline | allow | `document_id`, `outline_depth`, `nodes_returned` |
| `kb_get_deck_profile` | 返回 PPT 风格、母版、颜色、字体、布局摘要 | allow | `document_id`, `slides_sampled`, `profile_version` |
| `kb_retrieve_slide_examples` | 为新 PPT 规划找相似 slide 示例 | ask for large result | `query`, `library_id`, `slide_refs`, `thumbnail_refs` |
| `ppt_generate_plan` | 生成并验证 `pptx_plan.json` | ask | `task_id`, `slide_count`, `source_refs`, `schema_version` |
| `ppt_render` | 将 plan 渲染为 PPTX 和 QA 图片 | ask | `artifact_path`, `slide_count`, `qa_status`, `renderer_version` |
| `git_delivery_preflight` | 检查 Git 状态并提出分支/worktree 策略 | allow | `cwd`, `branch`, `base`, `dirty_summary`, `remote_status` |
| `git_delivery_finalize` | stage/commit/push/PR/merge 受控收尾 | ask | `files`, `commit_sha`, `pr_url`, `checks`, `merge_decision` |

### Connector 可见性
- 知识库、Office、Git、MCP、Skills 都必须作为用户可见 connector 暴露启停状态。
- 每次任务开始前 UI 必须展示本次启用的 connector、授权范围和可用 tools。
- 每次任务结束后 evidence pack 必须记录实际使用过的 connector、工具名、关键参数摘要和失败路径。
- 禁止把知识库、Office 或 Git 能力静默注入 prompt 后让用户无法审阅。

## PowerPoint 插件设计
### 本地服务
- 监听地址：默认 `https://127.0.0.1:<port>`，只绑定 loopback。
- 端点：
  - `GET /health`: 返回 version、build、service status、active pairing count。
  - `GET /build_info`: 返回 app version、commit、build time、service capability flags。
  - `POST /pair/start`: CodeFactory UI 生成一次性 pairing code。
  - `POST /pair/complete`: PowerPoint add-in 提交 pairing code，换取短期 token。
  - `POST /office/ppt/tasks`: 创建 PPT 生成任务。
  - `GET /office/ppt/tasks/{id}`: 查询任务状态、outline、artifact、错误。
  - `POST /office/ppt/tasks/{id}/approve-outline`: 用户确认 outline 后继续生成。
  - `GET /office/ppt/artifacts/{id}`: 返回 PPTX base64 或 artifact metadata。
- 安全要求：
  - CORS 仅允许已登记 Office add-in origin。
  - 所有非 health 请求必须带 pairing token。
  - token 过期、origin 不匹配、未配对、权限拒绝都必须返回可审计错误。
  - 本地服务不得暴露通用文件读写接口；知识库访问必须走 CodeFactory 权限和审计。

### Add-in UI
- Task pane 首屏：连接状态、选择知识库、当前演示文稿上下文、任务输入、生成/停止按钮。
- Outline 审阅：页标题、每页目的、引用来源、预计素材、保留/重写/删除 slide。
- 插入模式：
  - 插入为新 slides。
  - 替换选中 slides。
  - 生成新 PPTX artifact 供用户打开。
- 失败状态：本地服务未启动、证书/loopback 问题、未配对、知识库未索引、生成失败、PowerPoint API 插入失败。

## Git 交付设计
### 状态机
`idle -> preflight -> waiting_user_decision -> sync_base -> create_isolation -> execute -> verify -> finalize_review -> commit -> push -> pr -> checks -> merge_or_handoff -> cleanup`

### Preflight 决策
| Condition | Default action |
| --- | --- |
| 非 git repo | 允许执行非交付任务，但禁用 commit/PR/merge，标记 not deliverable |
| dirty tree | 展示文件摘要；默认建议创建 worktree 或用户手动处理，不自动 stash |
| 当前分支落后 base | `fetch` 后提示 pull/rebase 风险，需用户确认 |
| 多工具并行任务 | 默认创建 `git worktree`，避免共享 dirty tree |
| base branch 受保护 | 禁止直接提交到 base，必须走分支和 PR |
| required checks 未配置 | 允许 PR，但自动 merge 需要用户确认降级 |

### 分支命名
- 默认：`codex/<task-slug>-<yyyymmdd>`
- 冲突时追加短 id：`codex/<task-slug>-<yyyymmdd>-<shortid>`
- release hotfix 可按用户指定前缀，但必须记录理由。

### Finalize Gate
- 只有满足以下条件才可默认进入 commit/push：
  - 任务关联 Req ID 或明确用户任务说明。
  - 变更 diff 可展示。
  - 相关验证已运行并记录。
  - 失败验证有用户确认的降级理由。
  - 没有未归属的用户改动被 stage。
- 自动 merge 额外要求：
  - PR created。
  - required checks passed。
  - branch protection 未拒绝。
  - 用户开启 auto-merge 或本次明确授权。

### Git token 迁移要求
- GitHub/GitLab token 必须迁移到 OS credential store 或等价 secret store。
- `settings.json` 只保留 `token_ref`、provider、base URL、default repo 等非敏感配置。
- 迁移必须向后兼容旧 settings：首次启动发现明文 token 时写入 secret store，保存脱敏配置，并保留失败回滚路径。
- 迁移完成前不得扩展自动 push、PR、merge 能力。
- 现有文件型 secret store 也不得作为长期承载方案；迁移验收必须证明 token 不再出现在 `settings.json`、`keys.json`、日志、导出和测试快照。

## 测试矩阵
| Path type | Scenario | Expected result | Evidence |
| --- | --- | --- | --- |
| Primary path | 注册含 PPTX/DOCX/PDF 的知识库并完成扫描 | 文件被发现、解析、chunk 入库；失败文件单独记录 | SQLite 字段断言 + UI 索引状态截图 |
| Primary path | 在普通任务中调用 `kb_search` 和 `kb_get_chunk` | Agent 只收到 top-k 引用片段，UI 显示来源卡片 | tool call 记录 + retrieval event 抽样 |
| Primary path | 在 PowerPoint 插件中生成并插入 PPT | 本地服务配对成功，生成 PPTX，插入当前 deck | 录屏或截图 + artifact + 插入后 slide 计数 |
| Primary path | 开发任务完成 Git delivery | 创建隔离分支/worktree，验证通过后 commit/push/PR | git log/status + PR URL + checks evidence |
| Compatibility path | 旧 settings 无 knowledge/office/delivery 字段 | 启动后默认值补齐，不丢 endpoints、permissions、MCP、Git remote | migration/unit test |
| Compatibility path | 旧 SQLite DB 启动 | 新表被 idempotent 创建，旧 sessions/messages/tool_calls 可读 | DB migration test |
| Compatibility path | 旧 `.codefactory/memory.md` 项目记忆存在 | 仍可注入；知识库不覆盖项目 memory | system prompt test |
| Failure path | 知识库文件损坏或受权限限制 | 扫描不中断，失败文件记录错误并可重试 | ingestion test + UI 状态 |
| Failure path | 未配对网页调用本地服务 | 非 health 请求被拒绝，不泄漏知识库或文件路径 | local service auth test |
| Failure path | PowerPoint 插入 API 失败 | 保留 PPTX artifact，显示可恢复错误，不重复生成收费请求 | add-in integration test |
| Failure path | dirty tree 中有用户改动 | 不自动 stage/stash/revert，要求用户选择策略 | Git preflight test |
| Failure path | required checks failed | 不自动 merge，PR 保持打开，输出 blocker 和下一步 | Git remote mocked integration |
| Viewport path | 知识库页、引用卡片、PowerPoint task pane 窄宽度 | 关键文本、按钮、状态不重叠 | 1366x768 + narrow screenshots |
| Observation path | 索引、检索、PPT 生成、Git delivery | latency、error、route、artifact、source refs 可查询 | logs with redaction + SQLite events |
| Release path | 安装包内含 PowerPoint 插件向导和本地服务 | 安装后可启动服务、sideload manifest、完成主路径 smoke | `not live` 直到安装包和真实 PowerPoint 证据存在 |

## Evidence Pack Requirements
### Knowledge
- 知识库根路径、include/exclude 摘要、扫描开始/结束时间。
- 文档样本：PPTX、DOCX、PDF 各至少一个。
- 字段级断言：document hash、kind、chunk count、page/slide、metadata、status。
- 检索样本：query、filters、top_k、returned refs、source paths、token budget。
- 引用 UI 截图或结构化 evidence。

### PowerPoint
- Add-in manifest path、sideload 方式、Office/PowerPoint 版本。
- 本地服务 `/health`、`/build_info` 字段，pairing request/response 脱敏样本。
- 生成任务 request/response：task id、library ids、outline、approval、artifact id。
- 插入证据：插入前/后 slide count、目标演示文稿截图或录屏、失败恢复证据。
- 视觉 QA：PNG 渲染或等价截图，断言 slide count、标题存在、引用存在、无空白页、图片未缺失、关键文本不溢出。
- 安全证据：未配对请求、过期 token、origin 不匹配被拒绝。

### Git Delivery
- preflight JSON：cwd、base branch、current branch、dirty summary、remote status。
- isolation evidence：created branch/worktree path。
- verification evidence：命令、退出码、摘要、失败降级理由。
- commit/push/PR evidence：commit sha、PR URL、checks status、merge decision。
- user-change protection：stage 文件列表只包含本任务变更。
- stage ownership：每个 staged 文件必须能追溯到本任务 tool call、用户确认或显式 include；不得把其他工具留下的 dirty 文件一起提交。

### AI Collaboration
- context scope: 本次任务使用的知识库、文档范围、代码目录和外部资料。
- assumptions: 生成内容、引用、Git 策略、Office 插件限制的关键假设。
- review point: outline 审阅、diff 审阅、验证前/后、PR/merge 前。
- validation result: 检索质量、PPT 渲染 QA、Git checks、人工审批状态。

## 发布和兼容边界
- 在 PowerPoint 插件安装向导、HTTPS/证书处理、真实 PowerPoint 插入、安装包集成完成前，任何交付声明必须写 `not live`。
- 首期仅支持本地个人知识库，不承诺多人共享、远程同步或云端权限。
- 首期仅支持现代 Office XML 格式；老 `.ppt`、`.doc` 需要转换器或明确 blocker。
- 如果使用外部 embedding 模型，必须在 UI 显示数据将发送到模型提供方；默认应允许本地 embedding provider。
- Git 自动 merge 默认为关闭；用户明确开启前只创建 PR 和 evidence handoff。
