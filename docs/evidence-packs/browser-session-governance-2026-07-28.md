# 浏览器会话治理验收证据（2026-07-28）

## 现场根因

历史 `ai-fund-prod-verify` Playwright CLI 会话在 `run-code` 报错后跳过名义末尾的 `close`，遗留 headless Chrome 约五天并持续占用约一个 CPU 核心。普通 Chrome 与该进程树相互独立。

## CodeFactory 原生运行证据

从当前分支构建真实 CodeFactory 二进制，并运行：

```text
CodeFactory --browser-session-smoke <receipt.json>
```

验收回执：

```json
{
  "failure_detected": true,
  "lease_reclaimed_after_failure": true,
  "native_tool": "browser_session",
  "snapshot_ok": true,
  "status": "passed"
}
```

该 smoke 实际打开 `https://example.com`、获取 snapshot、注入不存在的元素引用，并验证 Playwright CLI 即使以退出码 0 输出 `### Error`，CodeFactory 仍按失败处理并回收租约。随后按 session id 和 `cliDaemon.js` 检查，未发现遗留 daemon。

## 自动化验证

- `cargo check --manifest-path src-tauri/Cargo.toml -p codefactory --lib --bin codefactory`：通过。
- `cargo test ... browser_session`：5 项通过。
- `cargo test ... rejects_direct_playwright_cli_in_favor_of_managed_sessions`：通过。
- 完整 Rust lib 测试：529 项通过、6 项忽略、0 项失败。
- `pnpm run build`：通过。
- 完整 Vitest：76 个测试文件、348 项测试全部通过。

## 跨 Agent 防线

- Claude Code `PreToolUse(Bash)` 对裸 `@playwright/cli` 命令返回 `deny`；受管入口可通过。
- Claude Code `SessionEnd` 实测关闭其 owner 租约，随后租约为空、daemon 消失。
- Codex Playwright wrapper 实测创建 `managed-codex-*` 会话；turn-ended bridge 实测关闭该租约，随后租约为空、daemon 消失。
- macOS LaunchAgent 已加载，每五分钟清理超过两小时的租约和 Playwright CLI daemon；只匹配 `playwright-core/lib/entry/cliDaemon.js`，不终止普通 Chrome。
