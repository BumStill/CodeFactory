# M198：信任模式解释器 heredoc 与审批理由

## 用户原始规格（需求与约束原文）

- R1 根因并修复：信任模式下，命令只在会话已登记工作区内读写文件（包括 python/node 等解释器 heredoc 改源码）时不得弹审批；M182 的危险清单（推送、改远端、破坏性 git、越界路径、外发网络等）照旧拦截。
- R2 审批意图必须落库「拦截理由 + 命中的规则」，界面审批卡片上显示人话理由；无法给出理由的拦截视为缺陷（fail-closed 仍可，但必须有理由）。
- R3 回归：把上面原文作为固定 fixture 加入测试（先失败）；再加一组「解释器 heredoc 写工作区文件 / 写越界路径 / 内含 git push」的对照，越界与 push 仍拦。
- R4 真实路径：e2e（合成会话、trusted）跑上面原文命令 → 零审批、文件被改写；附越界对照。

约束：task-template（harness + SDD + TDD）；合成 fixture；不改生产数据。授权交付（ceiling: merged）。Release-Urgency: immediate。

## Requirements Traceability

| Req ID | 最低证据 |
| --- | --- |
| R1 | heredoc 解析与文件操作证明测试；未知语法、符号链接、凭据、网络、子进程对照 |
| R2 | SQLite prompt_args_json 内持久化 rule/reason；审批卡片渲染断言与真实浏览器截图 |
| R3 | 固定事故 fixture，先红后绿；越界和 git push 保持拦截 |
| R4 | 合成工作区与 trusted 网关集成，真实解释器执行，断言零审批和内容改写；越界不执行 |

## Applicable Harnesses

Spec、Compatibility、Payload、Observation、Viewport、Release、AI Collaboration。合并不等于发版；安装版真实应用验收由编排方发版后完成。

## Implementation Notes

- 任务提供的复现 JSON 在 classify_extension_error 处包含 `…<truncated>`，缺少 Python 脚本结尾和 heredoc 终止符，不能作为可执行程序。保留原始截断证据，另用完整、合成的同形文件替换命令作可执行验收；不伪称截断命令执行成功。
- 安全证明必须基于完整 quoted heredoc 与解释器语法，不得仅靠搜索路径或危险单词来放行任意程序。不能证明时保留审批并记录原因。
- 后台操作入口沿用现有 dispatch / pending approvals，不依赖 GUI 点按。
- Context scope：可信 shell、登记工作区、审批持久化与卡片。Assumptions：仅证明受支持的静态文件编辑语法，不扩大外部执行权限。Review point：安全反例与真实文件结果。Validation result：待验证。
