# Trusted-mode permission prompts (M41)

## Background and user decision
Trusted mode means "the user trusts the agent to do whatever it needs inside this task's own workspace". Things that affect the outside world or the user's own data still need a confirmation. While waiting for confirmation, the task must not fail just because the user is away for a short while.

## Decision
In trusted mode, operations whose effects stay entirely inside the session's own managed workspace don't prompt. Operations that reach outside the workspace keep the existing confirmations. A confirmation request is never silently voided or recorded as "cancelled by the user" because nobody was around; once the user is back, they can still approve it and the task continues.

## Requirements Traceability

| Req ID | Requirement | Minimum evidence |
| --- | --- | --- |
| CF-TPP-R1 | In trusted mode, deleting, overwriting, or running scripts **only inside the session's own managed workspace** doesn't prompt for authorization | Table-driven tests: deleting a generated directory inside the workspace, overwriting files in the workspace, running scripts in the workspace → no prompt |
| CF-TPP-R2 | **Not weakened**: in any mode, operations that reach outside the workspace (deleting/writing paths outside the workspace or in the user's main checkout, paths that escape via `..` or symlinks, system settings, credentials, pushing directly to the default branch, etc.) keep their existing confirmation or refusal | Counter-example tests, at least one per category; tests must include path-escape cases |
| CF-TPP-R3 | When the session is already trusted, the prompt doesn't show "信任本会话并允许", and explains in one plain sentence why this action still needs confirmation | Component test + real-browser screenshot |
| CF-TPP-R4 | When nobody responds to a request in time, the task enters a "waiting for your approval" state that the user can see; once the user is back they can still approve it and the task continues; nothing already done is undone, and nothing is lost | Integration test: no response for a long time → user approves after coming back → task continues |
| CF-TPP-R5 | Records stay truthful: one that expired without a response is recorded as "not answered", never as "cancelled by user" | Test |
| CF-TPP-R6 | When the user switches to a session that has a request awaiting approval, the request is shown right away (evidence: 2026-10-09 15:35, session 06196246 had a pending request, switching over showed nothing until it expired, and the reissued one was the first to appear); the sidebar also marks which sessions are waiting for approval | Component test + one real-browser run of "switch away, then switch back" |

## Applicable Harnesses
Spec Harness; Viewport Harness (approval prompt, waiting state, light/dark); AI Collaboration Harness (touches the security boundary: the PR must list every changed rule that decides "does this prompt"; R2's counter-example tests are part of the review point).

## Test matrix
- Trusted mode: in-workspace delete/overwrite/run → no prompt.
- Counter-examples (any mode): outside the workspace, main checkout, `..` escape, symlink escape, credentials, system settings, pushing directly to the default branch → still confirmed/refused.
- Standard mode: existing behaviour unchanged.
- Nobody responds: wait long → approve later → continue; deny → the task explains plainly and stops that action.
- Viewport: prompt in trusted / standard mode, waiting state, light/dark.

## Primary User Path
用户在受信任会话里让 Agent 继续干活 → Agent 删除自己生成的目录 / 覆盖工作区里的文件 / 重跑工作区里的脚本 → **不再弹授权框**，任务连续执行。一旦某个操作走出本次会话的工作区（用户主 checkout、`..`、软链接指向外部、凭据、系统设置），仍然弹框。用户离开一会儿没人应答时，请求保持“等待你的批准”可视状态并可继续批准，而不是被判为“由用户取消”。

## Evidence Pack Requirements
- `cargo test --lib` 全量结果（含 `agent::trusted_workspace` 与 `agent::permission_gateway`）。
- `pnpm test` 全量与 `pnpm build`。
- PR 自验收表逐条对应 CF-TPP-R1..R6 的通过/未通过与证据。

## 兼容性和发布边界
- 仅影响权限判定与授权提示的等待语义；不改数据库 schema（复用既有 `permission_intents.expires_at`）。
- `full_access`（trusted）之外的模式行为不变；工作区根无法解析时行为不变（继续弹框）。
- 发布边界：本 PR `ceiling: pr_only`，只开 PR 不合并。

## Implementation Notes
- 新增 `src-tauri/src/agent/trusted_workspace.rs`：对 shell 命令做“仅限本会话工作区”的可证明性判定（`local_delete` / `local_write`）。任何解析不了的语法一律返回 `false`（fail closed）。
- `decide_permission_for_call` 增加 `workspace_root` 参数，仅在 `policy.full_access`（trusted）且判定为工作区本地时把 shell 的 `Ask` 提升为 `Allow`；这是本次唯一的放宽点。
- 网关 `DesktopPermissionGateway::authorize` 通过 `execution_workspace::latest_for_session` 解析本会话受管工作区根；解析不到就不放宽。
- 授权请求的等待改为两阶段：首个提示窗口结束后不再作废请求，而是延长持久 intent 的 `expires_at`（`PermissionIntentStore::extend_pending`）、重发同一 intent 事件并保持“等待你的批准”，再给一个长窗口；仍未应答才终态化为“未获答复”。
