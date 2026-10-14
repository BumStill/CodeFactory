# M8 / M28 / M84 验收证据（没人在跑的任务不许转圈；验收口径跟授权一致；能结案）

- 日期：2026-10-14
- 规格：`docs/specs/feature-specs/truthful-delivery-state.md`（"M8 / M28 / M84：验收口径与等待结案"一节）
- 任务：`task-ui-truth-m8-m28.md`（真机反馈 v1.88.1：23 个会话转圈、其中 21 个其实在等人）
- 数据：全部为合成数据（sqlite `:memory:`、临时 git 仓库、`src/acceptance/` 合成会话）。未读写生产数据库。

## 1. Req ID ↔ 测试 ↔ 结果

| Req ID | 要求 | 测试 | 结果 |
|---|---|---|---|
| CF-TRUTH-M84-R1 | `requested_acceptance` 与授权交付上限一致；`ceiling: merged` 时口径是 merged | `commands::chat::tests::m84_merged_authorization_does_not_infer_live_from_delivery_or_template` | pass |
| CF-TRUTH-M84-R2 | 历史误判的 live 任务幂等按 merged 结算（只从原始 root 请求的正向合并观察） | `agent::execution_workspace::tests::m84_historical_misclassified_live_settles_once_at_merged` | pass |
| CF-TRUTH-M84-R3 | `close_objective` 显式结案：守卫 + 审计 + 幂等（CF-CLOSEOUT） | `commands::chat::tests::m84_close_objective_is_guarded_audited_and_idempotent`；`src/lib/desktopDispatch.test.ts`（派单入口路由 + 非 `accepted` 拒绝） | pass |
| CF-TRUTH-M8-R1 | 只有真在跑的才转圈；等人的用静态标识并写明等什么；横幅不显示内部计时；无运行的会话不显示停止按钮 | `src/lib/sessionTurnState.test.ts`、`src/pages/Workspace/WorkspacePage.systemStop.test.tsx`、`src/components/SessionSidebar.test.tsx`、`src/lib/statusBanner.test.ts`（`M8 parked waiting`） | pass |
| CF-TRUTH-M28-R1 | 顶栏 PR 状态取 `delivery_runs` 最新记录并及时刷新 | `commands::git_remote::workspace_delivery_tests::m28_latest_delivery_run_supersedes_the_stale_session_pr_link`、真浏览器 `merged-beats-old-ci` | pass |

## 2. 命令与结果

```bash
# 前端全量（含 lightModeAudit 等静态门禁）
pnpm test                 # 147 files / 987 tests passed
pnpm build                # tsc + vite build 成功

# 后端全量（专用构建目录）
CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-m8m28" \
  cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast
# lib: 1591 passed / 0 failed / 9 ignored（既有跳过）；其余 test target 全绿

# 真浏览器验收（真布局 + light/dark 截图）
node scripts/verify-legacy-closeout-headless.mjs
# {"status":"pass","checks":["switch-away-back-retains-exact-approval","sidebar-waiting-mark",
#  "trusted-prompt-no-retrust","merged-beats-old-ci","light-dark"]}
```

## 3. 先看到失败（TDD 记录）

| 测试 | 旧实现下的结果 |
|---|---|
| `M8 parked waiting > says what it is waiting for …` | 失败：`expected '系统正在等待自动重试' to be 'PR #175 已合并，等你确认验收'`（旧横幅对没有排定重试的任务谎称"正在等待自动重试"） |
| `sessionTurnState > M8: authoritative idle suppresses stale streaming …` | 失败：残留 `streaming=true` 被当成在跑（侧栏一直转圈） |
| `SessionSidebar > shows parked waiting as static and offers explicit closeout` | 失败：无静态等待标识、无"确认验收 / 结案"入口 |
| `m84_merged_authorization_does_not_infer_live_from_delivery_or_template` | 失败：正文里的 "delivery" 命中 `live` 推断，`ObjectiveKind` 变成 Live |
| `m28_latest_delivery_run_supersedes_the_stale_session_pr_link` | 失败：旧数据源 `session_delivery_refs` 在 `updated_at` 更新后仍指向旧的 PR（顶栏停在旧 CI 结论） |

## 4. 真浏览器截图

`docs/testing/evidence/m8-m28-m84/`：

- `approval-light.png` / `approval-dark.png`：等待验收的会话在侧栏用静态标识「等待确认验收」并写出等待内容，**没有** spinner（`getByLabel('运行中', { exact: true }) === 0`）。
- `merged-light.png` / `merged-dark.png`：合并刷新后顶栏显示「已合并」，旧的 `CI 失败` 结论被压过（`merged-beats-old-ci`）。

## 5. 真机（安装版）验收

按任务分工，安装版里的点按、审计日志核对由编排方在发版后完成：

1. 打开侧栏，确认只有 objective 为 `active` 的会话转圈；`waiting_system` 的会话显示静态标识与等待原因。
2. 打开一个已合并、等待验收的会话，账户菜单 →「确认验收 / 结案」，确认会话从等待列表移出，且 `objective_events` 出现 `acceptance_confirmed`（`source=explicit_acceptance`）。
3. 顶栏 PR 徽标应在 15 秒内跟上最新结论，不再停在旧的 CI 状态。
