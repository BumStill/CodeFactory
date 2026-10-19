# M121：Windows 真浏览器 / 无头验收页间歇超时（家族审查）

## 1. 现象与证据

同一断言在 Windows runner 上间歇超时，重跑即过。12 小时内三次独立触发：

| 时间（本地） | PR / run | 失败目标 | 失败断言 | 重跑结果 |
|---|---|---|---|---|
| 2026-10-19 10:19 | #749 `scenario-execution` / 39143263152 | `execute-windows` 启动会话 | `Startup session probe` 未在 10s 内挂载 | 通过 |
| 2026-10-19 10:39 | #756 `check-rust` / 39184049777 | 真浏览器 `thin_browser_open_reaches_driver_without_receipt` | `browser_session.rs:1538`：Managed 会话存在但无页面内容 | 通过 |
| 2026-10-19 11:06 | #756 `scenario-execution` / 39189448369 | `execute-windows` 会话回交 | `main[Turn terminal convergence acceptance]` 10s 未挂载 | 通过 |

共同点不是"Windows 慢"，而是**同一个因果形状**：一个新创建的页面（Node 侧 `page.goto`、Rust 侧 `Target.createTarget`）返回时只代表请求已发出，不代表文档已提交并渲染；随后立刻读取 DOM / 断言地标，读到的可能是空的 `about:blank`。冷 runner 上这段窗口更宽，于是间歇失败；同一 job 内后续导航因为 Vite 模块已编译、文件已被 Defender 扫描过，窗口急剧变窄，所以重跑即过。

## 2. 归因与修复决策

| # | 需求 | 决策 |
|---|---|---|
| R1 | 家族级归因 | 归因到"导航提交前就读取"，而不是"把超时调大"。两个失败面各有一个确定性复现器：无头脚本侧用可注入的假 page 断言预算归属，Rust 侧用"先发响应头、延迟 750ms 再发 body"的 fixture。 |
| R2 | 统一修复 | `scripts/headless-acceptance-support.mjs` 提供 `gotoScenario` / `waitForAppBoot`：**boot 预算**（`APP_BOOT_BUDGET_MS`，默认 60s，可 `CF_HEADLESS_BOOT_BUDGET_MS` 覆盖）只覆盖"文档加载 + 应用外壳挂载"，**断言预算**（`ASSERTION_BUDGET_MS` = 10s）对启动后的每一次产品断言保持不变。boot 耗时会打印 `[acceptance] boot <ms>ms (budget <ms>ms)`，预算因此是被测量的，不是被猜的。超时会输出 DOM 轮廓、`document.body` 片段、console/pageerror/失败请求。 |
| R3 | Managed 浏览器语义 | `open` 在返回前等待文档 committed + 完成加载：轮询 `document.readyState`，要求 href 已提交且（`complete` 或有非空可见文本的 `interactive`），上限 20s；超时错误带上最后一次观测到的 `readyState/href/文本`。 |
| R4 | 重复验证 | `scenario-execution.yml` 新增 `execute-windows-soak`：带 `flake-soak` 标签的 PR 在同一 job 内把同一批 Windows 目标连续跑 5 次（第一次付冷启动，后四次复用同一构建缓存），任何一次失败即失败。默认 PR 不产生额外成本，也不改变任何必需检查。 |

### 预算不是"调大超时"

- 被替换的是**导航后的第一次等待**，语义是"进程启动完成"，不是产品断言。
- 断言预算仍是 10s；把产品交互也放宽会让"启动慢"掩盖真实的渲染回归。
- 每次 boot 都记录实测耗时；若某次 boot 真的逼近预算，日志会直接给出数字，而不是再攒一次重跑。
- 新增静态守卫 `src/acceptance/headlessReadiness.test.ts`：`page.goto`/`page.reload` 之后的第一处等待若仍用 10s 常量，测试失败。新脚本必须走 `gotoScenario`/`waitForAppBoot`。守卫放在 `src/` 下是因为 `pnpm test`（vitest）已经在每个 PR 的前端必需检查里执行它，不需要改动受信根工作流。

## 3. 覆盖范围

- `scripts/headless-acceptance-support.mjs`：新增 `observePage` / `describePageState` / `waitForAppBoot` / `gotoScenario` / `APP_BOOT_BUDGET_MS` / `ASSERTION_BUDGET_MS`。
- `src/acceptance/headlessReadiness.test.ts`：预算归属、诊断输出、家族守卫（由 `pnpm test` 自动执行）。
- 19 处 boot 等待改为共享 helper，涉及 15 个验收脚本（含本 family 点名的 startup-session、turn-handback）。
- `src-tauri/src/browser/chromium.rs`：`open` 读取前等待导航提交，等待分两段——先用 `Target.targetInfo`（不依赖执行上下文，不会被导航期间的上下文切换挂住）等提交，再用有界 JS 探针等文档就绪；任一探针超时 2s 就重试，因此失败信息是"最后看到的 url/readyState"而不是一个笼统的 30s 操作超时。
- `src-tauri/src/tools/browser_session.rs`：延迟 body 的确定性复现测试。
- 重复验证不放工作流改动：`.github/workflows/*` 属于受信 scenario gate 根，普通 PR 自改会被 `scenario-gate-pr` 拒绝（本次已验证该拒绝生效）。因此 R4 的重复证据通过在同一 PR 上多次重跑 `execute-windows`（每次都是全新冷 runner）取得，而不是新增 job。

## 4. 验收

1. `npx vitest run src/acceptance/headlessReadiness.test.ts` — helper 语义 + 家族守卫；变异测试（临时注入 `goto` 后 10s 等待）会让守卫按预期失败并报出行号。
2. `cargo test browser_open_waits_for_the_navigation` — 修复前失败、修复后通过。
3. 同一 PR 上 `execute-windows` 连续多次重跑全绿（每次冷 runner，PR 内证据）。
4. `verify-startup-session-headless.mjs`、`verify-turn-handback-headless.mjs` 在本机真实浏览器跑通，boot 耗时出现在日志里。
5. 重复冷启动实测（同一脚本连续 3 次）：`[acceptance] boot 36ms / 42ms / 35ms (budget 60000ms)` —— 60s 预算相对真实启动有一到两个数量级余量，10s 断言预算从来不够的结论由此得到数据支撑。
