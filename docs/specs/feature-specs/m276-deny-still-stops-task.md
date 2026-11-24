# M276：M269 没生效——拒绝一条审批后仍然以「阻塞总结」结束会话

## R1 证据：模型当时收到的拒绝文案是对的

会话 `afc40ac6-8c6a-42cc-ac0f-48573b106501`（M274 任务）在本机数据库里留下两条相邻记录：

| 时间 | role | 内容摘要 |
|---|---|---|
| 2026-11-11 00:24:11 | `tool` | `Tool call denied by the user. …` + `The denial covers this one operation only: the task is not cancelled; change approach and continue …` + `User note (why the user denied this; it is guidance, not an authorization): 请改…` |
| 2026-11-11 00:24:16 | `assistant` | `## 阻塞总结` / `停在哪一步：交付前校验 … 我读取注册表的那次调用被拒绝 … 尚未提交、未开 PR` |

也就是说：M269 定下的「只拒这一条、任务没取消、按说明继续」的文字与 note **都已经到达模型**。
所以本条不是文案没送到，而是下面两处判定没有接住它。

## R2 证据：结束会话的是结算层的措辞清单，不是权限决策

`agent/thin_derive.rs` 在终态推导里确实有 M269 的守卫（`reason == "最终回答已完成"`、无停止意图、
无真实完成事实、最新助手文本是阻断措辞 → 挂续接唤醒）。但它判断「阻断措辞」靠的是一张固定的
`BLOCKED_SUMMARY_MARKERS` 清单：`任务已停止 / 已停止本次 / 整单停 / 无法继续 / 不能继续 /
无法推进 / 被阻塞 / 审批被拒绝 / blocked`。

现场那段真实总结一个都不含：它的标题是 `阻塞总结`，正文是 `那次调用被拒绝`、`尚未提交、未开 PR`。
守卫因此没有触发，`最终回答已完成` 落成 `ended`——权限决策只拒了一条命令，会话却被结算层结束了。

同一处还有第二条本该兜住它的路径：`assistant_declared_unfinished`（M211）只认
`尚未完成 / 还没有完成 / 还没完成 / 任务未完成 / 将继续 / 会继续 / 自动继续`；现场写的是
`尚未提交、未开 PR`，同样漏过。

## R3 需求：拒绝后必须继续推进

- 拒绝文案要让模型知道：只拒了这条操作，任务没取消，应换安全路线**在本回合继续**；
  不要把一条拒绝升级成整单停，也不要用「阻塞总结」代替继续。
- 确实只剩用户才能决定的事时，只问一个具体问题，而不是写阻断总结。

## R4 非目标：不得削弱权限决策

只改文案与结算判定；`PermissionOutcome`/允许与否的判定、M265 的禁止绕过首句一律不动。

## R5 验证：真实网关 + 真实回合结算

- 网关侧：真实 `DesktopPermissionGateway` 走一次 `DeniedByUser` + note，断言实际交给模型的内容
  （经过落库前的脱敏）仍带 note 与「本回合继续」。
- 结算侧：把现场原话（`阻塞总结`、`那次调用被拒绝`、`尚未提交、未开 PR`）写进 `messages`，
  走真实 `thin_derive::refresh`，断言不是 `ended` 而是续接/等待，并落了真实唤醒。
- 反例：已经交付的回合、正常完成的总结，仍必须判 `ended`。

## R6 交付纪律

- 不改 `docs/testing/scenario-registry.json`（信任根）；缺映射时只追加 `docs/testing/scenario-mappings.json`。
- 修改只用 `edit_file`/`write_file`；查询用 `jq`/`grep`；本机数据库用 `sqlite3 -readonly`；
  构建目录与临时文件只在 `.codefactory-cache/` 下。

## 完成边界

- 本次只保证：同类「未交付 + 阻断措辞」的回合不再被判 `ended`，且拒绝文案不再鼓励整单停。
- 不新增用户可见开关，不改权限模式语义，不改发布节奏。
