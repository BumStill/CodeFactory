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

## Implementation Notes

### R1 只读复盘结论（2026-11-25）

只读查询 `/Users/leo/Library/Application Support/com.codefactory.app/codefactory.db` 与 `dispatch-audit.log`，未启动应用、未连接正式 `dispatch.sock`，也未操作 PR #1006、#1043、#1053、#1075：

- 会话 `bd4ba210-b480-4cec-b776-3992254d30f2` 是 `thin` 模式，`sessions.delivery_authorized=1`，`sessions.delivery_ceiling=NULL`，`release_urgency=default`。
- 该会话没有 durable `delivery_runs` 记录；#1075 的实际恢复依据是 `thin_delivery_refs`，其中 ceiling 为 `ThroughRelease`。因此首次和后续恢复的 armed 来源是全局默认 ceiling 经 thin 路径持久化为 `ThroughRelease`，不是会话内明确的 `pr_only`。
- 两条续接派单都保留了 `delivery_authorized=true`，同时正文明确记录人工已摘掉 armed、完成前不得再次交付。旧实现只读取布尔交付授权，没有持久化“改回草稿/摘掉 armed 即撤回合并授权”，所以恢复与系统唤醒仍会沿用 `ThroughRelease` 再次 armed。
- `Release-Urgency: immediate` 不是该 session 的结构化派单值；session 记录为 `default`。M285 保持 urgency 仅由结构化 `release_urgency` 字段进入交付，不从正文文字推断。

### 实现边界

- 在 thin 真实交付入口前统一计算 `min(会话授权, 本次请求)`。
- 复用 PR 前只读取得 `isDraft`、labels、远端 title/body；草稿或已授权后被摘标会持久化 `revoked:<previous ceiling>` 并清除 `sessions.delivery_authorized`，重启后继续拒绝，直到新的明确授权重新置位。
- 已有 PR 的远端 title/body 作为恢复输入，保留人工修改和 R 表；只允许现有治理组装逻辑做字段级补充。
- 未改权限判定、validator、workflow 或 `docs/testing/scenario-registry.json` 信任根；仅在 `docs/testing/scenario-mappings.json` 将新增 M285 交付文件最小映射到既有 `CXD-002`，不放宽任何 gate。
