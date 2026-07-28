# First-class delivery capability

Status: shipped (v1) · Owner: agent loop · Related: `agent/delivery.rs`,
`tools/delivery.rs`, `config/settings.rs` (DeliveryCeiling), the three system
prompts in `agent/mod.rs`.

## Problem

CodeFactory's agent had no notion of git delivery in its definition of "done".
The interactive TDD loop ended at step 6 "Report a summary"; the system prompts
never mentioned commit/push/PR/CI/merge/release. The only PR automation
(`auto_create_pr_if_configured`) was task-tree-only, opt-in, and opened a draft
PR without committing, pushing, waiting for CI, merging, or releasing. So the
interactive chat path had **zero delivery capability**: when a user's standard
was "open a PR, run CI, merge, release", the model improvised bash `git`
commands, hit the `bash=ask` permission gate, and — because its prompt said done
= artifact+tests+report — stalled, re-listing the missing delivery evidence in a
loop instead of executing it. Observed live: a session did all local TDD (tsc,
tests, build green) but never committed; the branch had 0 commits, no PR.

The fix is a product capability, not a one-off hand-delivery: CodeFactory itself
must be able to carry code work through delivery.

## Design

### Configurable ceiling — the user owns the boundary

`Settings::delivery_ceiling` (`DeliveryCeiling`, serde-default, backward
compatible like `SubagentIsolation`):

- `Off` — never auto-deliver (the tool still exists for explicit use).
- `PrOnly` — commit → push → open PR, then stop (manual review boundary).
- `ThroughCiGreen` — …+ poll CI to a conclusion.
- `ThroughMerge` — …+ merge (per `delivery_merge_method`).
- `ThroughRelease` (**default**) — …+ trigger a release, because user-visible delivery is the shipped artifact/update path.

The product default is `ThroughRelease`, matching the expectation that code work is done only when the user-visible artifact/update is live. Users can lower the ceiling in Settings when they want a manual PR/CI/merge stop.

### Hybrid provider — no `gh` dependency

Local ops (stage / commit / push) shell out to the `git` CLI, exactly like
`commands/git.rs` and `checkpoint.rs` — no new runtime dependency. Remote ops
(PR / CI / merge / release) go through the portable token+REST `git_remote`
layer (`RemoteGitClient` + `git_remotes` tokens) via the `DeliveryRemote` trait
(`GithubRemote` impl). `gh` is authenticated on some dev boxes but is **not** a
safe end-user assumption, so it stays out of the product path.

### Noise-safe staging — the structural guarantee

Delivery NEVER runs `git add -A`/`git add .`. `stage_scoped` runs `git add -u`
(stages tracked modifications/deletions, adds no untracked file) then adds only
untracked `??` entries that pass the built-in noise denylist (`.claude/`,
`CLAUDE.md`, `AGENTS.md`, `src-tauri/gen/schemas/`, `codex-worktrees/`,
`.codefactory/attachments/`, …) plus the user's `delivery_exclude_globs`. Local
junk can never be swept into a delivery commit.

### Idempotent / resumable state machine

`deliver()` walks steps up to the effective ceiling; each checks reality first:
nothing-to-commit is a clean skip, an already-open PR is reused via
`list_prs(head)` (never double-opened), CI-red/conflict/no-token are **blocked**
terminals with a clear message — never a loop, never a double-apply. Re-invoking
after a crash continues from the real git/PR state.

### Agent integration — killing the loop

The `deliver_changes` tool (`tools/delivery.rs`) exposes the capability; the
model calls it ONCE instead of improvising bash. `ExecCtx` carries a settings
snapshot so the tool reads the configured ceiling + remote tokens. Step 7
"Deliver" is appended to `SYSTEM_PROMPT` (interactive TDD loop) and
`SYSTEM_PROMPT_EXECUTE`: *"after the suite is green, call `deliver_changes` ONCE
… do NOT hand-run git … do NOT stop at a green build to describe a missing PR."*
`SYSTEM_PROMPT_AUTONOMOUS` is intentionally left unchanged — subagents run in
isolated per-task worktrees; session-level delivery is the scheduler's concern,
and a per-subtask PR would be wrong.

## Known limits / follow-ups

- **CI polling holds the turn** up to `delivery_ci_timeout_secs` (default
  1800s); on timeout it reports `pending` and the next call resumes. An async
  "come back later" variant is a follow-up.
- **`git push` credentials**: plain push relies on the machine's git credential
  helper. A token-URL host-matched push fallback (never logged/persisted) is a
  follow-up for machines without a helper.
- **Branch protection**: `ThroughMerge` on a protected `main` with required
  reviews correctly surfaces `merge_blocked` (405). Settings copy sets that
  expectation.
- **`ThroughRelease`** dispatches `auto-release.yml` via `workflow_dispatch`,
  which needs a token with `workflow` scope; a repo-only token surfaces a clear
  blocker.
- **Completion-gate enforcement**: this ships the prompt-level drive (kills the
  interactive loop). Hard gate enforcement in `agent-core` (refusing "done"
  without delivery evidence for non-interactive modes) is a follow-up — it would
  change `agent_contracts/execution_completion.md`'s pinned SHA and must land
  with the golden-hash update.
- **Dogfood**: once merged and the app is rebuilt, CodeFactory can deliver its
  own next branch (e.g. the stuck `codex/settings-hooks-remotes-tdd`) through
  this capability — the real "factory improves the factory" proof.
