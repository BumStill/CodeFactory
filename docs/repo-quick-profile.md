# CodeFactory Repo Quick Profile

## Task Entry
- Repository: `CodeFactory`
- 产品类型：Windows AI 编程 Agent 桌面客户端。
- 当前状态：已存在 Tauri 2 + React + TypeScript 基础聊天、模型列表、工具调用和权限流代码。
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

## 快速命令
- 治理基线：`python tools/governance/validate_repo_governance_baseline.py`
- PowerShell 包装：`powershell -ExecutionPolicy Bypass -File tools/governance/check_repo_governance.ps1`
- 长任务记录：`python tools/governance/validate_long_task_record.py --task-record-path <path>`

## 当前阻塞
- 发布通道、安装包签名、真实 OpenRouter 主路径和自动更新尚未完成。
- 浏览器主路径验证可覆盖前端构建和静态 UI；真实 Tauri 工具执行仍需要桌面运行环境、API Key 和代表性项目样本。
