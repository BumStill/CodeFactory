# CodeFactory Repo Quick Profile

## Task Entry
- Repository: `CodeFactory`
- 产品类型：Windows AI 编程 Agent 桌面客户端。
- 当前已验证公开版本：`v1.76.2`。一级「进化审查」、人工裁决和持久作业日志已存在；输入控件对齐与会话侧栏归属修正已经过本地真实 App、远程 GUI、公开 macOS DMG 和版本元数据验收。
- 日常任务先加载本文件、任务说明和 quick gate 结果。
- quick gate 标记 release、compatibility、observation、payload、viewport 或 governance-change 时，再加载 `docs/repo-governance-profile.md`。

## Load First
- `docs/repo-quick-profile.md`
- 当前任务说明或 `docs/specs/feature-specs/` 下的特性规格
- quick gate 输出

## Harness Triggers
| 触发条件 | 需要加载或追加 |
| --- | --- |
| 治理、validator、证据包、角色规则、规格层变更 | `governance-change`，加载完整 profile 和全局治理参考 |
| 生产发布、安装包、签名、自动更新、回滚、live defect | Release Harness + Observation Harness + Compatibility Harness |
| OpenRouter 协议、模型 route、历史会话、权限策略、旧配置 | Compatibility Harness |
| 文件读写、命令执行、图片/文件输入、会话导出、网关限制 | Payload Harness |
| 主聊天界面、模型选择器、权限弹窗、终端、diff、移动/窄屏 | Viewport Harness |
| AI 生成代码、多角色协作、关键假设不清 | AI Collaboration Harness |

## Minimum Evidence
- 本地治理变更：`python tools/governance/validate_repo_governance_baseline.py` 通过，或记录明确 blocker。
- 产品代码变更：先有失败测试或验收，再运行相关 unit/integration/browser/build。
- 工具调用路径：记录工具名、权限决策、实际 cwd、输入摘要、输出断言和失败路径。
- OpenRouter 路径：记录 provider/model route、请求字段、SSE/tool_calls 解析断言、错误处理。
- UI 主路径：目标视口截图或录屏，证明输入、审批、diff、命令输出和结果状态不互相遮挡。
- Release 路径：安装包或发布地址、build metadata、`/health` 或等价健康证据、回滚边界、主路径 live verification。
- AI Collaboration：context scope、assumptions、review point、validation result。

## Dev 实地验证权限基线
- CodeFactory 的本地 Dev、隔离 wrapper、临时 app bundle 及其他等价验证环境，在执行真实主路径前默认先启用并保存“信任模式（减少确认）”；agent 自行完成权限设置和授权，不把普通工具确认交给用户。
- CodeFactory Dev 与隔离 wrapper 的普通真实主路径验证默认使用 DeepSeek v4 Flash；只有被测行为明确涉及其他 provider、model 或 ChatGPT 登录链路时才切换，并在证据中写明。
- 只有验收目标本身是权限询问、拒绝、hook cancel 或权限绕过防护时，才临时切换到 ask/deny；该场景结束后恢复完全权限，并在证据中写明切换范围。
- 完全权限只用于当前已授权任务内的产品工具调用，不扩大部署、外发消息、账号、支付、交易、数据删除等外部或高风险操作权限。
- 并行任务存在时优先使用独立 identifier、数据目录和 app wrapper，避免改动或抢占其他任务的 Dev 环境。
- 本机锁屏后不得要求用户解锁或绕过 macOS 安全。UI 变更用 `pnpm test:evolution:headless` 继续执行真实浏览器布局/键盘门禁；PR、合并、刻意发版和安装包验证使用 CLI 与 GitHub runner。headless 不能替代发布 DMG smoke，DMG smoke 也不能替代功能断言。

## 快速命令
- GitHub 主分支门禁：`python3 tools/governance/manage_main_branch_ruleset.py validate|plan|apply|verify`
  - 设计入口：`docs/design/github-main-gates-business-design.md`、`github-main-gates-architecture-design.md`、`github-main-gates-ux-design.md`
- 跨 worktree 共享 Cargo 缓存：`pnpm cargo:shared -- <cargo arguments>`
  - **裸 `cargo build` / `cargo test` 不走共享缓存**，会在当前 worktree 另起一份数 GB 的 `target/`。长会话优先用上面这条。
- 清理已完结的 worktree：`pnpm worktrees`（只报告）/ `pnpm worktrees:clean`（删已合并且干净的，并清 7 天未动的构建产物）
  - 本仓库一律 squash 合并，`git merge-base --is-ancestor` 会把已合并分支判成未合并；脚本靠「是否存在已合并 PR」来判定，不要用裸 merge-base 自己写清理。
- PR 合并后清理自己的目录：从其他 checkout 运行 `pnpm worktrees:closeout -- --path <绝对路径> --apply`；该命令只接受 GitHub 已合并 PR，兼容 squash merge。
- Rust 快速回归：`pnpm test:rust:fast -- <test filter>`
- 治理基线：`python tools/governance/validate_repo_governance_baseline.py`
- 场景测试治理：`python tools/governance/validate_scenario_test_governance.py`
- 场景权威源：`docs/testing/scenario-registry.json`；产品 `feat/fix` PR 必须声明 `Scenario-Test: <IDs>`。
- PowerShell 包装：`powershell -ExecutionPolicy Bypass -File tools/governance/check_repo_governance.ps1`
- 长任务记录：`python tools/governance/validate_long_task_record.py --task-record-path <path>`

## 当前阻塞
- Evolution Agent 的任务效果 Eval case 仍缺少跨项目统一可执行 oracle；Phase 4 首版只把确定性的激活安全回归作为自动激活硬门禁，不把它包装成任务成功率提升证明。
- release-facing 完成仍需 PR+CI、安装包以及真实 CodeFactoryDev/发布版本的主路径证据；浏览器和 mock 不能替代桌面工具执行。
