# M285：交付工具必须遵守请求的 ceiling，不得擅自解除草稿、打 armed 或重写 PR 正文

## 现场（会话 bd4ba210，PR #1075）
- 11-17：任务未完成，会话自述请求 pr_only，交付工具按 through_release 开 PR 并打 `merge-queue: armed`。
- 11-18 11:51：PR 为草稿、正文自述未完成，再次被打 armed，且带 `Release-Urgency: immediate`。
- 11-25 10:13：PR 被转为 ready、打 armed，描述被重写；需求尚未通过真机验收。随后由人工恢复。
- #1071：恢复交付把 PR 标题/正文重写成恢复说明，丢了 R 表。

## 规格（放到 `docs/specs/feature-specs/m285-delivery-ceiling-draft.md`）
- R1：只读复盘 bd4ba210 的 delivery 记录与 objective 授权，写清每次 armed 的真实来源（会话授权、恢复路径、续接派单的 delivery_authorized 还是默认 ceiling），不要先假定原因。
- R2：实际 ceiling = min(会话授权, 本次请求)；恢复、重启续跑、系统唤醒不得提高它。pr_only 永不打 armed、永不发版。
- R3：已是草稿的 PR，交付工具不得自动转 ready 或打 armed；人工改回草稿或摘掉 armed 后，视为用户撤回合并授权，需用户新的明确授权才能恢复，重启后仍有效。
- R4：推送新提交或恢复交付时，不覆盖已有的 PR 标题/正文；需要补字段时只做局部追加，R 表不得丢失。用户或外部改过的正文以远端为准。
- R5：`Release-Urgency: immediate` 仅来自任务派单的 release_urgency，不从模型文本推断。
- R6：拒绝时给出可读原因（谁撤回、当前上限是什么），不得反复重试。
- R7：不放宽任何权限判定，不动信任根。

## 测试（先看到失败，走真实交付入口）
pr_only 不打 armed；through_release 授权 + pr_only 请求 → pr_only；草稿不被解除；人工摘标后重启续跑不重新 armed；恢复交付保留正文和 R 表；反例：明确授权 merged 的非草稿 PR 仍正常 armed。

## 约束与交付
探针用隔离 HOME，不连正式 dispatch.sock。不碰 PR #1006、#1043、#1053、#1075。PR 正文用 `R | 是否完成 | 证据` 列 R1–R7；未全部完成则保持 draft、不打 armed。授权交付。

Release-Urgency: default
