# Content-addressed resume journal for parallel task dispatch

Status: shipped (v1) · Owner: agent scheduler · Related:
`agent/journal.rs`, `docs/design/subagent-worktree-isolation.md` (the isolation
boundary this journal's replayability boolean builds on).

## Problem (three gaps)

1. **Orphans.** A crash left rows at `status='running'`;
   `list_pending_tasks_for_session` returns only `'pending'`, so the orphan was
   never re-dispatched and `is_task_ready` blocked its children forever.
2. **No content addressing.** A completed task was skipped purely by status —
   a changed brief/model/tools/cwd neither invalidated it nor cascaded to its
   dependents; conversely there was no way to resume a large interrupted run
   without redoing verified work.
3. **"Done" was a flag, not evidence.** Nothing checked that a completed
   task's output was still on disk: a checkpoint revert or an out-of-band
   `git checkout` silently orphaned the status.

## Design

Every verified completion writes a **journal row** (`task_journal`, separate
from live `task_runs` state on purpose: retries NULL out `task_runs.result`,
so the journal is the durable last-known-good record — never deleted, only
marked `'stale'`).

**Two-level content address.** `local_digest` = SHA-256 over length-prefixed,
domain-tagged frames of the task's resolved inputs (title, ORIGINAL
description, effective model, sorted tools, cwd, acceptance criteria, task
context, isolation mode). `dispatch_key` = `local_digest` folded with the
sorted `(dep_id, dep_dispatch_key)` pairs — an upstream change recursively
changes every descendant's key. Journal-less dependencies (legacy) get a
stable `legacy_sentinel(id)`. No timestamp/PID enters any digest.

**Resume = two phases at `run_session` start** (plus a session-agnostic boot
sweep in `ensure_schema`, mirroring evolution_jobs recovery):

- **Phase A — orphan reconciliation.** Owner identity is `owner_pid` +
  `owner_start_token` (PID-reuse safe). Dead-owner `'running'` rows reset to
  `'pending'`; a worktree task caught in `state='merging'` is decided
  exactly-once by reality: if its durable patch `git apply --reverse --check`s
  cleanly the merge landed → finalize to completed, else reset. Live owners
  are never touched. An in-process panic is covered separately by a
  `DispatchGuard` (RAII) that resets the row on unwind.
- **Phase B — revalidation (`plan_resume`).** One topological pass recomputes
  each completed task's current `dispatch_key`; it is **restored** iff a
  `'done'` journal row matches, `merge_applied=1`, no upstream is dirty, and
  the **presence gate** passes (worktree: reverse-apply-check the durable
  patch copy; `no_changes`: trivially present; shared: trusted, covered by the
  proactive revert hook). Everything else resets to `'pending'` with a typed
  `InvalidationReason`, and dirtiness propagates so dependents re-run strictly
  after their rebuilt parents (`is_task_ready` unchanged). Cycles fail closed.
  Legacy completed tasks without journal rows are preserved (non-regression)
  and **backfilled** so future changes are detected.

**Dispatch is a CAS claim** (`pending → running` + owner identity in one
UPDATE) — two schedulers, even in two processes sharing the DB, can never
double-dispatch a task.

**Checkpoint interaction.** `revert_checkpoint` now calls
`invalidate_on_revert(session, checkpoint.created_at)`: the whole-tree restore
wipes every edit made after the checkpoint, so every task completed at/after
it resets — once, at revert time; the cascade falls out of the next resume.

**Worktree interaction.** Before `merge_back`, a durable `'merging'` intent
row is written; on `Applied` the merge-back patch is copied to
`app_data/task-journal-patches/` **before** worktree cleanup reaps it (it is
the presence-gate evidence at every later resume); on `Conflict`/`Err` the
journal goes `'stale'` (never replayable). Completion flips journal `'done'` +
`task_runs 'completed'` together.

## UI + lock-safe verification

`plan_resume` emits one `resume_summary:{session}` event (`ResumeReport`) and
per-task `task_restored` events. `TaskDashboard` shows a banner (已从缓存恢复
N / 重新执行 M / 恢复中断任务 K, with per-task reason chips) and a 已缓存
badge (key-short tooltip) on restored rows. The banner is a pure function of
the report — which is what makes the surface verifiable with the screen
locked: `resume-journal-acceptance.html` mounts the real `TaskDashboard`
against `mockIPC` and replays a real `resume_summary` event;
`scripts/verify-resume-journal-headless.mjs` (CI: `pnpm test:resume:headless`)
asserts counts, reason chips, badges, both viewports, no overflow, zero page
errors, and writes a receipt with `interactive_desktop_required: false`.

## Known limits

- Shared-mode out-of-band edits (e.g. a manual `git checkout` between runs)
  have no per-task diff to presence-check under parallelism; the revert hook
  covers the app-mediated path. Documented blind spot.
- Drift that occurred *before* this feature existed cannot be detected for
  legacy tasks (no completion-time snapshot); they are skipped-and-backfilled
  rather than mass-invalidated on upgrade.
- `attempt_count` is not consumed by a crash (a crash is not an attempt);
  cross-crash counts are cosmetic.
