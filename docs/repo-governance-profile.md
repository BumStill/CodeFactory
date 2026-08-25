# CodeFactory Repo Governance Profile

## Governance Sync Metadata
- Adopted Governance Version: `2026-08-25`
- Global Governance Sources:
  - `codex-delivery-governance`
  - `harness-taxonomy.md`
  - `multi-project-harness-standard.md`
  - `governance-change-control.md`
  - `governance-propagation.md`
- Propagation Policy: `profile-review`
- Last Governance Review Date: `2026-08-25`
- Local Governance Owner: `CodeFactory maintainers`

## Repository Basics
- Repository name: `CodeFactory`
- Product type: `Windows AI 编程 Agent 桌面客户端`
- Primary runtime: `Tauri 2 + Rust + React + TypeScript + Vite`
- Default working directory: `D:\CodeFactory`
- 当前状态：Tauri 2 + React + TypeScript 基础聊天和工具调用代码已初始化，发布通道尚未完成。

## Surface Map
| Surface ID | Type | Entry path or URL | Responsible role | External |
| --- | --- | --- | --- | --- |
| desktop-ui | desktop webview | `src/` | end_user | no |
| tauri-backend | local backend | `src-tauri/src/` | system | no |
| tool-runtime | local file/process tools | `src-tauri/src/tools/` | system | no |
| openrouter-api | external api | `https://openrouter.ai/api/v1` | provider | yes |
| sqlite-store | local database | `%APPDATA%\CodeFactory\sessions.db` | system | no |
| credential-store | OS credential store | Windows Credential Manager | system | no |
| release-artifact | installer/update | MSI/NSIS/GitHub Actions，计划中 | release | yes |
| governance | local governance | `docs/`、`.codex/governance/`、`tools/governance/` | planning | no |

## Critical Roles
| Role ID | Name | Primary goal | Critical path |
| --- | --- | --- | --- |
| end_user | 本地开发者用户 | 通过 CodeFactory 完成受控 AI 编程任务 | P1 |
| system | CodeFactory 本地系统 | 执行模型、工具、存储、权限和审计路径 | P1 |
| release | 发布维护者 | 构建、签名、发布和回滚 Windows 安装包 | P3 |
| provider | OpenRouter 或模型提供方 | 提供模型响应、tool_calls 和用量信息 | P1 |

## Role Gate
- 规划 / 系统工程角色负责 Requirements Traceability、Primary User Path、Applicable Harnesses、测试矩阵和证据包要求。
- 开发角色只实现已批准 Req ID，记录兼容性影响和最小 patch。
- QA / 验收角色独立验证真实主路径，拒绝结构-only、mock-only、HTTP 200-only 证据。
- 发布 / 运维角色在 release-facing 任务中负责部署、配置、回滚、build metadata 和 live verification。
- 非平凡变更默认使用后台角色 sub-agent；若当前工具或上层指令不允许，最终输出记录跳过原因。

## Primary Path Directory
| Path ID | Path name | Surfaces | Release blocking |
| --- | --- | --- | --- |
| P1 | AI 编程主路径：打开应用、选择 cwd 和模型、发起任务、审批工具、查看 diff/测试结果 | desktop-ui + tauri-backend + tool-runtime + openrouter-api + sqlite-store + credential-store | yes |
| P2 | 纯对话路径：选择模型、发送消息、流式展示、记录成本和历史 | desktop-ui + tauri-backend + openrouter-api + sqlite-store + credential-store | yes |
| P3 | Windows 发布路径：构建、签名、安装、启动、健康检查、回滚 | release-artifact + desktop-ui + tauri-backend | yes |
| P4 | 治理路径：规格、角色门禁、证据包、基线 validator | governance | yes |

## Command Map
### Local verification
- governance baseline: `python tools/governance/validate_repo_governance_baseline.py`
- governance PowerShell wrapper: `powershell -ExecutionPolicy Bypass -File tools/governance/check_repo_governance.ps1`
- typecheck: `pnpm typecheck`，脚手架创建后启用
- lint: `pnpm lint`，脚手架创建后启用
- build: `pnpm build` + `cargo build`，脚手架创建后启用
- unit test: `pnpm test` + `cargo test`，脚手架创建后启用
- integration test: `cargo test --test <name>`，接口稳定后启用
- browser test: `pnpm exec playwright test`，前端主路径出现后启用

### Release
- deploy: `pnpm tauri build` 后通过 GitHub Actions 发布安装包，尚未启用
- rollback: 回退到上一安装包或自动更新通道上一版本，尚未启用

## Platform Map
| Type | Platform | Notes |
| --- | --- | --- |
| desktop | Windows 10/11 | x64 首发，arm64 后续 |
| runtime | Tauri 2 / WebView2 | Windows 原生桌面壳 |
| frontend | React + TypeScript + Vite | 主聊天、权限弹窗、diff、终端 |
| backend | Rust async runtime | OpenRouter、工具、SQLite、权限、审计 |
| deploy | GitHub Actions + Windows installer | 计划 MSI/NSIS 和签名 |
| storage | SQLite + Windows Credential Manager | 会话落 SQLite，API Key 落凭据管理器 |

## Environment and Service Map
| Environment | Service name or ID | Domain or address | Notes |
| --- | --- | --- | --- |
| local-dev | CodeFactory desktop | `D:\CodeFactory` | 当前仅治理基线 |
| production | CodeFactory installer | 未启用：产品脚手架和发布通道尚未创建 | 发布规格落地后补充真实地址或安装包通道 |
| external | OpenRouter | `https://openrouter.ai/api/v1` | 模型路由、SSE、tool_calls、usage |

## Release Verification Map
| Field | Value | Notes |
| --- | --- | --- |
| HealthPath | 未启用：当前用安装后启动 smoke 作为桌面健康证据 | 桌面应用可用等价物：启动、主窗口、配置读取、模型列表或本地健康命令 |
| BuildInfoPath | 未启用：发布实现前必须加入版本/build 信息展示或命令 | 安装包版本、commit SHA、构建时间、签名状态 |
| WarnAfterMinutes | 8 | deployment latency warning threshold |
| FailAfterMinutes | 20 | deployment latency blocking threshold |
| PrimarySmokePath | P1 | 安装后跑通 AI 编程主路径的 smoke |

## High-Risk Trigger Directory
### Payload Harness
- 本地文件读写、diff、会话导出、图片粘贴、命令输出截断、SSE chunk 聚合。
- 验证必须包含字段级断言和实际处理 route，例如 `ToolRegistry -> read_file`、`PermissionPolicy -> ask/allow/deny`。

### Viewport Harness
- 主聊天界面、权限弹窗、模型选择器、工具调用卡片、diff viewer、终端区域。
- 验证目标至少覆盖 1366x768 和窄屏窗口，证明关键按钮和状态不重叠。

### Compatibility Harness
- 旧会话数据库、旧项目配置、旧权限策略、模型 provider 差异、OpenRouter 响应格式差异。
- 任何持久化 schema 或配置语义变更必须提供迁移和回退证据。

### Release Harness
- 安装包签名、WebView2 依赖、自动更新、配置目录、凭据读取、Windows 权限。
- 发布声明必须有安装后启动和 P1 或 P2 现场证据。

## Evidence Pack Requirements
### Minimum before release
- Primary User Path 截图或录屏。
- OpenRouter 请求/响应或模拟替代样本，包含 model route、tool_calls route、usage/cost 字段断言。
- 权限弹窗和危险命令拒绝路径证据。
- SQLite 持久化或迁移证据。
- 安装包版本、commit/build metadata、签名或未签名风险说明。
- 回滚边界和失败恢复动作。

### Changes that always require screen evidence
- 主聊天输入、流式输出、工具调用卡片、权限确认、diff viewer、终端、模型选择器、设置页、发布后启动页。

## Repository-Specific Constraints
- API Key 只能进入 Windows Credential Manager，不得写入日志、settings、会话导出或测试快照。
- 默认工作目录限制在用户选择的项目根目录；Full access mode 只能绕过用户配置的 allow/ask/deny 提示，开启状态必须在 UI 中可见并带风险说明。
- 所有写文件和命令执行都必须可审计，用户可看到参数、权限状态、输出和结果状态。
- OpenRouter route 差异必须通过 provider 抽象或 normalize 层隔离，不得散落在 UI。
- 治理基线或前端构建通过不得解释为真实桌面主路径已发布可用。

## Change Control Rules
- 全局 Harness taxonomy、object model、selection rules、minimum evidence、validator contract、propagation behavior 不得在本仓库先改。
- 需要调整这些全局规则时，先进入 `CODEX_HOME` 级 `codex-delivery-governance` 和 `governance-change-control.md`。

## Propagation Response
- Current expected action on global update: `profile-review`
- How this repo records adoption work: 更新 `AGENTS.md`、本 profile、`.codex/governance/baseline-manifest.json`，并运行 baseline validator。
- How drift is detected locally: `tools/governance/validate_repo_governance_baseline.py` 检查版本、文件清单和 marker。

## Reusability Classification Log
| Capability | Classification | Rationale | Candidate global skill |
| --- | --- | --- | --- |
| CodeFactory 产品规格和主路径 | repo-local | 依赖本项目技术栈、Windows 桌面体验和 OpenRouter 集成 | no |
| Harness 基线 validator | global-market source mirrored locally | 来源于全局 `codex-delivery-governance`，本地仅镜像执行 | no |
