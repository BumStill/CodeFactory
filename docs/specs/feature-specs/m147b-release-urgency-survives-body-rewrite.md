# M147b：会话登记 release_urgency=immediate，但 PR 正文最终没有 Release-Urgency（M137 未贯彻到底）

## 现场
- 会话 `21c93806`（M143）`sessions.release_urgency=immediate`，PR #818 合并时正文不含
  `Release-Urgency:`；合并后未自动切版，编排方手动 dispatch。#800/#802 前例相同（当时 M137 未上线），
  **#818 发生在 M137 上线之后**。
- 推测：会话后续用 `gh pr edit --body` / 再次 deliver 覆盖了正文，或 thin 交付更新正文时没有重新附加 trailer。

## 根因（代码事实）
1. **thin 交付的载体只有 PR 正文**：`delivery_thin::run` 在 `commit -m <title>` 之后才解析 urgency，
   提交信息里从不写 trailer；`prepare_thin_pr_body` 只把 trailer 写进 PR 正文。
   正文一旦被覆盖，trailer 在 git 历史里**没有任何副本**，合并队列无从回落。
2. **工具层不设防**：`bash` 工具对 `gh pr edit --body/--body-file` 原样执行，模型（或任何后续回合）
   可以用一条命令抹掉 trailer，交付工具感知不到。
3. 合并队列的 `squashMessage` 本身已有「正文无 trailer → 用提交 trailer」的回落（M122），
   但因为第 1 条，回落源一直是空的。

## 规则
- **R1 任何经 CodeFactory 发出的 PR 正文更新都要保证会话 `release_urgency` 对应的 trailer 存在于最后一段**，
  模型的覆盖不能把它抹掉（`immediate` 不得被降为无）：
  - R1a thin 交付在**提交信息**里也写 `Release-Urgency:`（从 `sessions.release_urgency` /
    调用参数解析出的同一值），使 trailer 在 git 里有持久副本；
  - R1b `bash` 工具拦截 `gh pr edit --body <text>` 与 `gh pr edit --body-file <path>`，执行前把会话
    trailer 补进被写入的正文（`--body-file` 用临时副本，不改用户文件），并在工具输出里记一条审计行。
- **R2 合并队列合并前若 PR 正文无 trailer，用提交信息的 trailer 回落补上**（与 M122 gate-dance
  回落一致）：`squashMessage` 产出的 squash 正文必须带上 `Release-Urgency:`。
- **R3 测试（先失败）**：
  1. 会话 `release_urgency=immediate` 交付后，提交信息带 `Release-Urgency: immediate`；
  2. 交付后执行 `gh pr edit --body x` → trailer 仍在（工具层拦截后 `gh` 实际收到的正文仍带 trailer）；
  3. 二次 deliver 换正文 → trailer 仍在；
  4. 队列合并时 PR 正文缺失 trailer、提交信息有 → squash 正文带 trailer。

## Requirements Traceability

| Req ID | 要求 | 最低证据 |
|---|---|---|
| M147b-R1a | thin 交付提交信息携带 trailer | `delivery_thin` 单测：`commit_message` 含 `Release-Urgency: immediate`，且会话 urgency 为空时不写 |
| M147b-R1b | 拦截 `gh pr edit --body/--body-file` 并补回 trailer | `pr_body_guard` 单测（正文/文件/无关命令/幂等）+ 真执行假 `gh` 的集成断言 |
| M147b-R2 | 队列回落用提交 trailer | `scripts/merge-queue.test.mjs` 的 `squashMessage` 用例 |
| M147b-R3 | 上述场景先失败后通过 | 先跑测试见失败，再实现见通过 |

## Applicable Harnesses
- Spec Harness：本条规格。
- Release Harness：`Release-Urgency` 直达 release 计划（`tools/release/plan_release.py`）。
- Compatibility Harness：不改 PR 正文契约、不改 validator；`gh` 与非 `gh` 命令行为不变。
- AI Collaboration Harness：模型可发起的覆盖动作被工具层兜住。

## 测试矩阵

| 场景 | 测试 |
|---|---|
| 会话 immediate → 提交信息带 trailer | `delivery_thin::tests::m147b_*` |
| 无 urgency → 提交信息不带 trailer | `delivery_thin::tests::m147b_*` |
| `gh pr edit --body` 补 trailer | `tools::pr_body_guard::tests::m147b_*` |
| `gh pr edit --body-file` 用临时副本补 trailer | `tools::pr_body_guard::tests::m147b_*` |
| 无关命令 / 已带 trailer → 不改写 | `tools::pr_body_guard::tests::m147b_*` |
| 真执行假 `gh` 收到带 trailer 的正文 | `tools::pr_body_guard::tests::m147b_*`（执行真实 shell） |
| 队列正文缺失 → squash 带 trailer | `merge-queue.test.mjs` |

## 实现说明（不改变需求）
- 合并队列运行在 GitHub 托管 runner 上，**读不到本机 `thin_delivery_refs` / `sessions` SQLite**。
  因此「按 thin_delivery_refs→sessions.release_urgency 回落」在实现上分成两段：
  交付时刻把 `sessions.release_urgency` 落进提交信息（R1a，git 持久载体），
  队列时刻用提交 trailer 回落（R2，已有 M122 逻辑，本次补测试）。两段合起来等价于规格要求的链路。

## 证据
- `src-tauri/src/tools/delivery_thin.rs`、`src-tauri/src/tools/pr_body_guard.rs`、
  `src-tauri/src/tools/bash.rs`、`scripts/merge-queue.mjs`、`scripts/merge-queue.test.mjs`。
