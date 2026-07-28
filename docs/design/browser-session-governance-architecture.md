# 浏览器会话治理：架构设计

## 组件

`browser_session` 是 CodeFactory 原生工具，不把 daemon 生命周期交给任意 shell 文本。

```text
agent -> browser_session tool -> BrowserSessionManager -> Playwright CLI session
                                    |                     |
                                    |                     +-- unique session id
                                    +-- lease registry ----+-- close/delete-data
                                    +-- startup janitor
```

## 核心不变量

- 会话 ID 由 CodeFactory 生成，且携带 `codefactory-` 前缀和任务标识。
- 每次操作刷新租约；租约包含 session、任务、最后活动时间。
- `close` 是幂等的。工具执行失败也会在失败路径关闭新建会话。
- CLI 即使以退出码 0 返回 `### Error`，manager 也按失败处理并立即关闭会话；不能只信进程退出码。
- `bash` 检测到裸 Playwright CLI、`playwright-core` daemon 或旧 wrapper 时拒绝执行，并指向 `browser_session`。

## 回收策略

- 正常路径：agent 调用 `browser_session.close`。
- 任务结束：任务生命周期调用 manager 清理该任务会话。
- 崩溃兜底：应用启动时读取 lease；所属进程已退出或超过租约阈值的会话由 CLI `close` 回收；另一仍存活的 CodeFactory 实例不会被误关。
- 不扫描或终止无 CodeFactory 前缀的任意 Chrome 进程。

## 验证

- 单元测试：识别并拒绝裸 CLI；会话 ID 与租约过期规则。
- 集成测试：模拟 CLI 失败时租约被移除并执行关闭。
- 真实运行：发布二进制的 `--browser-session-smoke <receipt.json>` 创建受管会话、注入异常并验证租约与 daemon 均被回收。
