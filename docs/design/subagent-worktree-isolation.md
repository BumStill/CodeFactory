# Subagent worktree isolation

Status: shipped (v1, opt-in) · Owner: agent scheduler · Related: `docs/BACKLOG.md`
(picked from the 2026-07-15 openJiuwen benchmarking entries), CF-EVO resume
journal (planned follow-up, interacts with the isolation boundary defined here).

## Problem

Parallel subagents share one working directory, guarded only by per-file locks
(`tools/file_lock.rs`). Locks prevent single-file write collisions but not
cross-file inconsistency: two tasks editing related files interleave freely,
and a failed task's half-done edits stay in the user's tree next to a
succeeded task's work. Our own repo development already mandates sibling
worktrees for risky slices (AGENTS.md) — the product should practice what the
factory preaches.

## Design

New setting `subagent_isolation` (`shared` default | `worktree`), plus
`max_parallel_tasks` (1–8, default 3) replacing the hardcoded scheduler cap.

With `worktree` mode, per dispatched task:

1. **Create** (`agent/worktree.rs::create`): `git worktree add -B
   codefactory/task-<id> <app-data>/task-worktrees/task-<id> HEAD`. Worktrees
   live under the app data dir, never inside the project — the user's `git
   status` stays clean. The task cwd is remapped to the same relative path
   inside the worktree. Stale leftovers from a crashed run of the same task id
   are pruned and replaced. Session brief (`_codefactory_brief.md`) and
   `.codefactory/` (minus `evidence/`, `worktrees/`) are snapshot-copied in so
   the subagent keeps project memory.
2. **Run**: the subagent brief cwd and the verification plan both point into
   the worktree. Retries reuse the same worktree, so partial progress carries
   across attempts exactly like shared mode.
3. **Merge back** (`merge_back`, only after verification passes): stage
   everything except the snapshot-copied context, commit on the task branch
   with a fixed identity, take `git diff --binary base..HEAD`, then apply onto
   the user's tree — `git apply --check` first, so the merge is
   **all-or-nothing**. CodeFactory never commits to the user's branch; changes
   arrive as ordinary uncommitted edits, same as shared mode. Merge-backs are
   serialized per session (one `git apply` at a time per checkout).
4. **Settle**: on success the worktree, branch, and patch are removed. On
   merge conflict the task is downgraded to failed and the branch + patch are
   preserved (the error message names both) for manual recovery; the user's
   tree is untouched. Failed tasks keep their worktree for inspection.

## Fallbacks and boundaries

- Non-git cwd, or a repo without a usable HEAD → per-task fallback to shared
  mode with a `task_progress` note. Nothing hard-fails.
- The shared session brief's cross-task "Task Results" updates land in the
  parent copy only; worktree snapshots don't see siblings' results
  mid-session. Acceptable v1 — results-sharing was already best-effort.
- Two concurrent *sessions* on the same repo still race merge-backs (the
  serialization lock is per session). Same exposure as today's shared mode.
- Dependency dirs (`node_modules/`, `target/`) are not present in a fresh
  worktree; verification plans that need them will install or fail visibly.
  This is the honest cost of isolation and a reason the mode ships opt-in.

## Interaction with the resume journal (next P0)

The journal's cache key can now include the isolation mode and the merge-back
outcome: a journaled "done" in worktree mode is only replayable if its diff
was applied. The all-or-nothing merge makes that a clean boolean.
