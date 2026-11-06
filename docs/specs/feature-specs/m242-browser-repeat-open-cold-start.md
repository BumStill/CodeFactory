# M242：Windows 冷启动时，连续两次打开同一网页，偶发只留下一个浏览器会话

## 现场（2026-11-05，CI run 42089585821，`check-rust`，windows-latest）
- 测试 `tools::browser_session::tests::browser_open_reads_the_same_content_when_repeated` 失败。
- 失败断言是 `assert_eq!(cleanup, 2)`：`left: 1, right: 2`。当时的行号是 `src\tools\browser_session.rs:2098`。
- 重跑后通过，所以每次撞上都要多等一轮 CI，并拖住合并队列。

## 现在不知道的事
清理数是第一个断言，排在 `first.status`、`second.status` 之前，所以失败时看不到两次 `open` 各返回了什么。只有一个会话可能有几种原因：某次 `open` 失败了；某次 `open` 的会话没有登记到清理表；或者清理时漏了一个。其中前两种可能是真实产品缺陷，不能预设是测试问题。

## 规格（放到 `docs/specs/feature-specs/m242-browser-repeat-open-cold-start.md`）
- R1：先让失败能说明原因：断言顺序改为先看两次 `open` 的状态和内容，再看清理数，失败信息带上两次返回的内容。
- R2：找到根因。可以用人为延迟或故障注入，确定性复现「只留下一个会话」。把结论和证据写进规格。
- R3：如果是产品缺陷，修产品：连续两次 `open` 必须各有一个可被清理的会话，或者明确返回失败，不能留下没人管的浏览器进程。
- R4：如果确认只是测试夹具的时序问题，修夹具，并在规格里写明证据。
- R5：不许隔离或跳过这个测试，不许靠放宽断言、加长固定等待来让它变绿。
- R6：不改 `docs/testing/scenario-registry.json`、`tools/governance/`、`.github/` 这些信任根。

## 测试
- 确定性复现用例：修复前失败，修复后通过。
- 这个测试在本机连跑 20 次全过；PR 里写明命令和结果。
- Windows 上的结果以 PR 的 `check-rust` 为准。本机是 macOS，不能代替 Windows 证据，要在 PR 里说明。

## 验收
R2 的根因有证据；上面的测试全过；CI 全绿。

## 交付
授权交付。PR body 写 `Scenario-Test`，按任务模板收尾。

## Implementation Notes（不改变上述要求）

### 根因与证据边界
旧夹具 `for _ in 0..8` 串行 accept 后无超时地 read；Chromium 可以先建立一个不发送请求的预连接，导致第二次 open 的 GET 排队而无人处理。有限连接计数还会把预连接/favicon 消耗算作页面请求。旧夹具以 `drop(JoinHandle)` 脱离线程，不会停止服务。

确定性传输回归 `repeat_fixture::tests::idle_preconnect_does_not_block_real_requests` 先建立并确认服务端已接受的空连接，再发送独立 GET；移植旧夹具时 2 秒后失败（`WouldBlock`），并发处理后同一命令通过。原重复打开测试也在两次真实 open 之间注入相同空连接，仍要求两次 Done、两份目标正文、正文一致、cleanup=2；不增加产品超时，不跳过或放宽断言。并发夹具限定每个连接的读写时间，忽略空请求，结束时 join 服务及工作线程。

产品现有 `ChromiumDriver::open_authorized` 在 goto、settle 或读取失败时调用 teardown；工具层失败会显式返回错误并移除创建失败的 lease。本次没有修改产品路径。**原 Windows 历史输出没有两次 open 的返回值，不能证明历史事件唯一原因就是预连接；已证明的是夹具自身的确定性饥饿缺陷，不把推断写成历史事实。**

### 验证矩阵与需求追踪
| Req | 最低证据 / 当前结果 |
| --- | --- |
| R1 | 原测试先检查两次状态与内容，所有断言带两次返回，最后 cleanup=2 |
| R2/R4 | 空预连接独立回归 red→green；真实 open 注入保留在 check-rust 执行 |
| R3 | 产品 goto/settle/read 失败 teardown 与工具显式 Error 路径审阅；无产品修改 |
| R5 | 双会话和正文断言不削弱，无新增 skip，无增加固定等待 |
| R6 | 信任根零改动 |

Applicable Harnesses：Spec、Compatibility、Observation、AI Collaboration；Release Harness 仅覆盖合并交接，不自行发版；无 UI/Payload 变更。

### 本机结果与未闭合项
- 独立 rustc 传输回归：修复前失败，修复后通过。
- `cargo test --manifest-path src-tauri/Cargo.toml --lib tools::browser_session -- --nocapture`：29 项报告通过，其中 **3 项真实浏览器用例因已有 macOS 嵌套沙箱探针提前返回**，不能视为真实浏览器通过。
- **本机 20 次真实浏览器执行尚未取得**；当前环境实测 `sandbox-exec ... /usr/bin/true` 返回 `sandbox_apply: Operation not permitted`，不绕过沙箱，不把 20 次跳过计为通过。Windows 结果必须由 PR check-rust 提供。
- 任务要求的隔离目录 Rust workspace 全量已执行：存在非本任务失败，包含路径含空格导致 provider hook 拆词、Unix socket 路径超过 SUN_LEN、沙箱内工具执行限制；不修改无关代码来掩盖。
- `pnpm test`：166 文件、1117 测试通过；`pnpm build` 通过。
- Python 3.11 统一 local scenario gate、仓库治理基线、Scenario/README 校验通过。

AI Collaboration：context scope 为浏览器工具及合成 HTTP 夹具；assumptions 为预连接引起串行阻塞但不宣称历史唯一根因；review point 为保留两次独立成功和清理语义；validation result 如上。按任务模板不调用 delegate_tasks，当前测试/修复紧耦合，未拆分并行修改。

真机验收由编排方发版后完成：在安装版连续 open 同一合成页面两次，核对独立 session_id、两份正确正文及 close 后实际进程退出。当前 not live，无 Spec Amendment；本机真实浏览器 20 次与 Windows CI 未闭合，不宣称验收完成。
