# M126 — 合并队列必须被每一个「可合并」信号唤醒

## 问题（真实事故）

2026-10-20，#768 的状态是 **armed + CLEAN + 必需检查全绿**，但队列 20+ 分钟没有运行：

| 本地时间 | 事件 |
|---|---|
| 04:10:44 | `merge-queue: armed` 打标 → 队列运行一次，因检查仍在跑而退出 |
| 04:24:13 | CI `workflow_run` 完成 → 队列运行（失败者打 `merge-queue: blocked`） |
| 04:24:29 | `merge-queue: blocked` 打上（记录当时的 head） |
| ~04:39 | 修复推送 + CI 全绿 |
| 04:40 | `merge-queue: blocked` 被摘掉 |
| 04:40 → 05:00 | **没有任何队列运行**，只能人工 `workflow_dispatch` |

根因不是队列逻辑，而是**触发面**：

1. `pull_request_target` 只订阅了 `labeled`，并用 workflow `if:` 过滤 `merge-queue: armed`。
   摘掉 `merge-queue: blocked` 产生的是 `unlabeled` —— 没有任何触发器监听，
   「解除阻断」这个**恰好让 PR 变为可合并**的信号被丢掉了。
2. 给**已经 armed** 的 PR 再打一次标签是 GitHub 的 no-op，不产生 `labeled` webhook。
   于是「等 CI 绿了再 arm」这条路径可以一直睡到下一个 10 分钟 cron。

## 决策

- **触发面按语义扩到 `labeled` + `unlabeled`**，过滤 `merge-queue: armed` 与
  `merge-queue: blocked`（后者含 `<!-- merge-queue:blocked ... -->` 变体）。
- **过滤逻辑从 workflow `if:` 移到 `scripts/merge-queue.mjs` 的 `resolveWake()`**，
  workflow 第一步执行 `node scripts/merge-queue.mjs wake --event ... --label ...`，
  后续步骤以 `steps.wake.outputs.run == 'true'` 为门。这样规则**可单测**，
  且 workflow 与规则不会各写一份而漂移（M78 的 workflow 形状测试同步更新）。
- **打标后显式唤醒**：`arm_merge_queue`（`gh` 路径与 REST 路径）在打标成功后
  `workflow run merge-queue.yml`（REST 为 `POST .../actions/workflows/merge-queue.yml/dispatches`）。
  这是 best effort：打标本身已成功，token 缺 `actions: write` 时仍由 cron sweep 兜底，
  不会让交付失败。
- 非标签事件（`schedule` / `workflow_run` / `workflow_dispatch`）一律照旧 drain。

## 安全考量（R2）

- 仍然使用 `pull_request_target`：workflow 从 **默认分支** checkout 可信队列工具，
  只执行 `node scripts/merge-queue.mjs wake`（纯判定，不读 PR 内容，不 merge）。
- 不新增权限：`contents: write` / `pull-requests: write` / `actions: write` / `checks: read`
  与改动前一致；真正的合并仍在 `drain` 步骤，由可信 runner 执行。
- 候选 PR 仍然无法运行自己的队列逻辑（`ref:` 断言保持：只能是 default branch，不能是 head/PR ref）。
- 并发组不变（`merge-queue`，`cancel-in-progress: false`），不会出现并发合并。
- 非队列标签的 `labeled`/`unlabeled` 事件在 `wake` 步骤即停止（数秒），不进入合并路径；
  `unlabeled: merge-queue: armed`（停止任务时撤 arm）也会 drain，`drain` 对「无 armed PR」一次 listing 即退出。

## 验收

- `pnpm test:merge-queue`：`resolveWake` 覆盖 armed / blocked（打标与摘标）/ 无关标签 /
  cron 与 `workflow_run`，并断言 workflow 执行的是被测试的谓词、trusted-ref 未变。
- `scripts/merge-queue.mjs wake` 写 `GITHUB_OUTPUT`（`run=true|false` + `reason`）有单测。
- Rust：`delivery_thin` 在「已 armed 再 arm」路径必须 `dispatch`（`already_armed_rearm_still_wakes_the_queue`）。
- 真实证据：本 PR 合入后由交付工具 arm 自身，`labeled` 事件必须在 1 分钟内拉起一次
  `merge-queue` 运行（对照 #768 的 20 分钟空窗）。
