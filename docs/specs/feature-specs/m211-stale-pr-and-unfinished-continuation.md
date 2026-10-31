# M211：续接会话做到一半被「上一单 PR 已合并」结束（M206 未覆盖的分支）

Release-Urgency: immediate

## 现场（v1.91.65，会话 5ee1876f）

10-30 09:20 用户在**同一个会话**里续接新任务 M196（新 objective / 新工作区 / 无 PR）。
10-30 09:24 会话被写成 `ended`，reason `PR #917 已合并；工作区已清理，无需收尾`。
PR #917 属于**上一单** M194，与 M196 无关；M196 实际零进展。用户看到的是
「任务刚开始就被判完成」，且此后不再有任何自动推进。

## 根因（thin 状态推导，两处独立缺口）

1. **终态事实没有按 objective / 工作区收敛**。`derive_candidate` 里
   `thin_delivery_refs` 是 `session_id` 主键的**会话级**引用，旧 objective 的
   PR #917 一直留在里面。`terminal_pr` / `completed_external` /
   `workspace_recovery_budget.settled_reason` 三条终态事实只要命中就结束会话，
   完全不看「该 PR 的分支是否还是本单的工作区」。新的 M196 工作区是 `active`、
   没有任何 PR，却被上一单的合并事实结束。
2. **回合自己说「尚未完成」也不影响判定**。M196 那一轮助手明确写了
   「任务尚未完成」。`derive_candidate` 的终态推导把「最新一条助手最终回答」
   当成 `最终回答已完成`，再叠加缺口 1 的 `completed`，直接 `ended`。
   系统没有任何自我续接唤醒兜底，等于把「我说还要接着做」翻译成「任务结束」。

M206 只覆盖了「用户在旧合并事实之后**又发了新用户消息**」这一条路径
（`m206_new_user_outranks_old_merge_and_settlement`）；本次事故里最新一条消息是
**助手**的续接回答，所以 M206 的守卫没有命中。

## 需求

- **R1 终态事实只看当前 objective / 最新工作区**：PR 合并、closeout 完成、
  settled 结算等终态事实，只有在其分支仍是本会话**当前工作区**（
  `state in (active, allocating, delivering)`）时才算本单终态。上一单 objective
  的 PR 状态与本单无关；最新工作区没有 PR 且未完成 → 不得 `ended`。
- **R2 助手声明未完成必须自我续接**：最新一条助手最终回答明确表达
  「尚未完成 / 会继续」时，回合结束后必须挂一条**真实 pending 唤醒行**
  （`wakeup_schedules`，`source=agent_decided`），状态为 `waiting_wake`，
  不得 `ended`。按助手消息时间戳去重，同一轮只挂一条；连续续接按指数退避，
  不产生无限 60 秒唤醒。
- **R3 反例固定成 fixture**：同一会话两个 objective —— 旧 PR 已合并 + 新工作区
  无 PR + 助手说未完成 → 不得 `ended`；并纳入 M206 的终态写入者不变量测试。
- **R4 修复现存**：启动/周期扫描时，把「因上一单 PR 合并而被误结束、且最新助手
  回答声明未完成」的会话重开，并挂**新的**续接唤醒（沿用 M209 的重开语义：
  新唤醒 id，不复用旧收尾唤醒）。

## 实现

- `thin_derive::stale_delivery_scope`：会话级判定——存在交付引用分支，
  当前有活工作区（active/allocating/delivering），且没有任何活工作区等于该分支
  → 该 PR 事实属于上一单。无活工作区（全部 closed）时保留原有语义，
  真收尾不受影响。
- `derive_candidate` 在读终态事实处统一按 `stale_delivery_scope` 过滤：
  `settled_reason`（含 `PR #`）、`terminal_pr`、`completed_external`、
  `unfinished_delivery`、以及「等 PR #N」等待分支与 `ensure_pr_monitor`。
- `thin_derive::assistant_declared_unfinished` + `continue_state`：按最新助手
  最终回答的内容判定未完成声明，按消息时间戳落一条 `thin-continue-<session>-<at>`
  pending 唤醒（退避 60s→1920s，上限 3 次后仍退避而非询问用户）。
- `reconcile_running_liveness`：额外挑选 `state='ended'` 且 reason 含 `已合并`、
  交付范围已过期、最新助手回答声明未完成的会话重新 `refresh`（R4）。

## 证据等级与验收

- `cargo test --lib agent::thin_derive::tests::m211`（新增 fixture，先红后绿）。
- M206 家族不变量：`m206_terminal_writer_family_preserves_unanswered_user`、
  `m206_new_user_outranks_old_merge_and_settlement` 保持绿。
- 真机主路径：同会话续接新任务后，旧 PR 合并不得结束会话；助手说未完成时
  `wakeup_schedules` 出现 `thin-continue-*` pending 行。
