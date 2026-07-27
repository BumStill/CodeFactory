# 模型运行时控制面长任务记录

## Basics

- Task ID: CF-MRC-20260727
- Title: 会话级模型策略、OAuth 恢复与惰性凭据读取
- Feature spec: `docs/specs/feature-specs/model-runtime-control-plane.md`
- Related Req IDs: CF-MRC-R1..R18

## Completion Standard

- Done means: R1..R18 有失败先行测试、真实 App 成功/边界路径、PR+CI、merge、正式 release
  和公开安装包证据。
- Blocked means: 同一外部阻塞有连续证据，且无安全的本地、headless 或 GitHub runner 替代。

## Current State

- Current phase: 实现、全量自动化与隔离桌面 App 验收完成，进入 PR+CI 与正式发布。
- Current checkpoint: 会话策略、OAuth 恢复、惰性凭据、图片能力门禁和 route journal 已实现；
  真实 App 额外发现并修复“端点默认覆盖会话模型”和“首选/自动执行语义相同”两个问题。
- Next owner: 当前实现线完成 PR+CI、merge、release 和公开产物验收。
- Updated at: 2026-07-27
- Live status: `not live`

## Completed Items

- [x] 核对现有 OAuth、会话模型、failover、Keychain 和图片预览链路。
- [x] 明确 Settings 默认与会话运行时分层。
- [x] 明确 fixed/prefer/auto 与“下一轮生效”语义。
- [x] 明确共享、可恢复 OAuth flow 和手动验证入口。
- [x] 明确按候选惰性读取凭据与结构化错误。
- [x] 建立 CF-MRC-R1..R18 Requirements Traceability。
- [x] 失败先行测试：session migration/config isolation。
- [x] 失败先行测试：OAuth start/open/status/cancel 与 AUTH_EXPIRED 会话恢复 UI。
- [x] 失败先行测试：CredentialBroker lazy/singleflight/cache。
- [x] 失败先行测试：能力资格与图片不移除。
- [x] 实现后端、前端和兼容迁移。
- [x] 前端全量：77 文件、336 tests；TypeScript 与 production build 通过。
- [x] Rust workspace 全量：715 passed、0 failed、6 ignored。
- [x] 隔离 `CodeFactoryModelRuntimeDev` 实地验证设置默认策略、已有会话
  `prefer -> fixed -> auto`、Settings 隔离、历史图片内联预览、历史 `auth_expired`
  恢复卡。
- [x] 实地发现并以失败测试修复：打开已有 `gpt-5.5` 会话时端点默认模型覆盖会话模型。
- [x] 失败测试后区分策略：`prefer` 每回合先尝试用户首选；`auto` 可按短期健康状态预选。

## Remaining Items

- [ ] PR+CI、merge、release、公开产物验收。

## AI Collaboration

- context scope: sessions DB/commands、route planner、OAuth、secrets、stream error、Settings、
  ModelPicker、chat store、MessageList。
- assumptions: 旧会话默认 fixed；OAuth browser open 不是完成证据；当前回合不可变；device
  code 在后端具备正式支持前不显示。
- review point: 独立规划、架构和 QA 角色只读复核；主实现线拥有唯一编辑权。
- validation result: 全量前后端自动化和隔离真实 App 主路径通过；实际 OAuth 授权点击未执行，
  以避免在验收环境创建新的持久账号授权，start/open/copy/cancel 由后端状态测试和组件测试覆盖；
  发布链待完成。

## Baseline Observation

- `cargo fmt --all --check` 在未修改的 `main` 基线上会要求重排大量无关 Rust 文件。本变更只对
  本次修改的 Rust 文件执行 `rustfmt`，避免制造全仓格式化噪音；`git diff --check` 通过。

## Stop Boundary

- 不在文档、单元测试、PR、merge 或 workflow 启动后停止。
- 只有公开安装产物的真实路径通过，或出现有证据的外部 blocker，才允许结束任务。
