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
