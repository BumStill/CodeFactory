# 超大工具输出导致会话中断：恢复任务

## 状态

`completed`（2026-07-27）

## 现场证据

- 故障版本：`v1.66.1`。
- 进程仍存活，数据库可读；不是桌面进程崩溃。
- 最新失败 route attempt：ChatGPT `gpt-5.5`、`prefer`、`CONTEXT_OVERFLOW`。
- 失败前最近工具结果：`grep`，2,298,886 个字符。
- 现有压缩已释放约 56,798 tokens，但因为超大工具结果位于最近半区且会话只剩两个用户回合，紧急重试仍溢出。

## 交付范围

- [x] 用生产形状写失败测试并确认旧实现失败。
- [x] 为 `grep` 增加单行和总输出边界。
- [x] 为最近超大 tool/assistant replay 增加 head/tail 兜底压缩。
- [x] 完成模块、workspace、治理和构建验证。
- [x] 用隔离生产形状历史完成真实 App/ChatGPT 恢复验证。
- [x] PR 通过 CI 并合并。
- [x] 刻意发布公开版本并验证安装包/updater。

## 约束

- 不修改生产数据库中的原始失败消息。
- 不用切换模型掩盖上下文溢出。
- 不接触主 checkout 中用户拥有的 `.codefactory/` 与 `codex-worktrees/`。
- 验收遵循 `docs/specs/feature-specs/tool-output-context-bounds.md` 的 CF-CTX-R1..R8。

## 已完成验证

- Agent loop：35 passed。
- Desktop Rust：522 passed，6 ignored，0 failed。
- Vitest：336 passed。
- `pnpm build`：TypeScript 与 Vite production build 通过。
- Governance baseline：pass。
- 隔离真实 App：`chatgpt / gpt-5.5 / prefer` 在含 2,298,889 字符旧工具消息的合成会话中返回 `RECOVERY_OK`，用时 5.3 秒。
- 恢复后的 route attempt：`succeeded`，`output_started=1`，`side_effect_started=0`。
- 恢复后上下文 UI：约 15K / 258K（6%）。
- SQLite 中原超大消息恢复前后均为 2,298,889 字符，证实 provider replay 压缩未改写原始历史。
- PR [#224](https://github.com/BumStill/CodeFactory/pull/224) 全绿并合并，merge commit `4a81de2f36229f909d568fd3248b8b0c252b010b`。
- 公开版本 [v1.66.2](https://github.com/BumStill/CodeFactory/releases/tag/v1.66.2) 已发布；macOS、Windows、签名文件和双平台 `latest.json` 均可公开下载。
- Release run `30267151061` 的 macOS、Windows、finalize 和匿名下载后的 published macOS GUI 验证全部通过。
- 本机正式 updater 从 `v1.66.1` 升级到 `v1.66.2` 并自动重启；清理 updater 重启竞态产生的无窗口残留进程后，生产 App 保持单实例。
- 真实故障会话在 `v1.66.2` 中可加载，原错误记录保留，`chatgpt / gpt-5.5 / prefer` 与输入入口可用；未替用户擅自继续原开发任务。
