# AI Coding OS 控制面业务设计

## 背景

CodeFactory 过去的定位是本地 AI 编程工作台：打开项目、选择模型、让 agent 读写代码、运行命令、提交交付。用户现在明确要求 CodeFactory 不再只作为 OpenClaw 的执行层，而要把 AI Coding OS 的控制面能力也落到产品内。

因此 CodeFactory 的新定位是：

> 单机 AI Coding OS：把规则、记忆、能力、工作流、交付和证据做成本地可见、可审核、可执行的系统。

## 目标用户

- 个人开发者或小团队负责人，同时使用 Codex、Claude Code、浏览器、GitHub、Office 和本地自动化工具。
- 需要多个 agent 并行开发，但不希望规则、记忆、分支状态、发布状态分散在不同工具里。
- 需要“已完成”能被证据证明：测试、PR、CI、release、真实 app 行为和证据包。

## 业务目标

| ID | 目标 | 验收方式 |
| --- | --- | --- |
| AICOS-B1 | 用户能在 CodeFactory 内看到当前项目的控制面状态 | 有专门入口展示 Authority、Memory、Capabilities、Delivery 四类状态 |
| AICOS-B2 | 分散的规则 surface 被显式呈现 | 控制面列出 `AGENTS.md`、`docs/specs`、项目 `.codefactory/specs`、git hook、release cadence 等是否存在 |
| AICOS-B3 | 记忆不再只是聊天缓存 | 控制面展示 pending/accepted/rejected learning events，作为 memory proposal lifecycle 的 v1 状态 |
| AICOS-B4 | 技能、MCP、知识库、hooks、Git remote 统一成能力视图 | 控制面展示 capability 数量、启用数量和风险提示 |
| AICOS-B5 | Git/发布交付门禁可见 | 控制面展示当前分支、dirty 状态、sync gate 文件是否存在、当前 checkout 是否启用该 hook、release workflow、latest release 标记 |

## v1 范围

本次落地的 v1 是“控制面快照”，目标是把已经存在的系统能力变成一个统一、可审计的产品面。

包含：

- 新增 AI Coding OS 控制面页面。
- 聚合当前项目的 Authority surfaces。
- 聚合 learning events 的 proposal 状态。
- 聚合 Skills、MCP、Knowledge、Hooks、Git remotes 的 capability 状态。
- 聚合 Git branch、dirty tree、sync hook 文件与本地 `core.hooksPath` 配置、release workflow 的 delivery gate 状态。
- 提供测试覆盖，确保控制面能从真实项目状态生成快照。

不包含：

- 不在本次实现完整规则编辑器。
- 不在本次实现自动 promotion/review 工作流。
- 不在本次实现云同步、团队权限、多租户。
- 不在本次替代现有 Profile、Skills、Settings、Specs 页面；控制面先作为系统总览。

## 产品原则

1. **控制面先可见，再自动化。** 用户必须先看见规则、能力、门禁和证据的来源。
2. **长期事实必须可审核。** memory proposal 只能显示为 proposal，不能直接成为权威规则。
3. **能力默认可控。** Skills/MCP/Knowledge/Git/Hook 都必须有启用状态和风险提示。
4. **交付不只看代码。** 发布完成需要 PR/CI/release/artifact 状态，而不是本地测试单点通过。
5. **OpenClaw 是迁移参考，不是运行依赖。** CodeFactory v1 自己呈现控制面，未来可导入 OpenClaw 历史规则。

## 成功标准

- 用户从 Home 能进入“AI Coding OS”控制面。
- 打开 CodeFactory 项目时，控制面能看到本仓库的规则、spec、hook、release、capability 和 git 状态。
- 没有项目时，控制面也能展示全局能力状态并明确缺少项目上下文。
- 单元测试覆盖后端快照逻辑和前端渲染。
- 本次变更通过治理验证、前端测试、构建、PR CI，并发布到新版本。
