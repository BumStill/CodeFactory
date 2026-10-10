# Feature Spec: Automatic closeout after a task is merged (CF-CLOSEOUT)

Task ID: CF-CLOSEOUT. Scope of this spec: CF-CLO-R1 – CF-CLO-R7.

> 2026-10-10: R8–R10 (build-cache reclaim / disk guard / usage view) moved to the
> dedicated spec `task-build-cache.md` (CF-BLD-R2–R4). This task covers R1–R7 only.

### Background and user decision
Once a task is merged, the user should not have to do any cleanup, and should not
be misled by "still failed / still running / CI failed" states. Any unfinished
content must be kept and stated clearly; nothing may be deleted silently.

### Decision
Once a session's PR is merged (whether by CodeFactory or externally), the app
settles the result and cleans up the workspace on its own. Before cleaning, it
confirms that everything is contained in the merged result; anything not
contained is kept and the user is told.

### Requirements Traceability

| Req ID | Requirement | Minimum evidence |
| --- | --- | --- |
| CF-CLO-R1 | Within a bounded time after the session's PR is merged (merged by CodeFactory or externally), the delivery record shows "merged" plus the merge commit; the session result shows "已合并", no longer failed / recovering / running | Integration test: both paths (merged by itself / external merge) |
| CF-CLO-R2 | Once merged, and only after confirming the workspace content is fully contained in the merged result: remove the worktree, delete the local branch, delete the remote branch if it still exists | Test: deletes only the exact branch / exact directory; containment check covers squash merges |
| CF-CLO-R3 | Content not contained in the merged result (uncommitted, unpushed, extra commits) is not deleted; the session tells the user in plain words what's left and where it is | Test + wording passes the internal-vocabulary guard |
| CF-CLO-R4 | When a PR is closed without merging: don't delete anything; the session shows "PR 已关闭，改动保留在 …" | Test |
| CF-CLO-R5 | Existing leftovers: after upgrading, apply R1–R4 once to historical workspaces (clean what's merged and contained, keep the rest); never touch the user's main checkout or worktrees the app didn't create | Synthetic data test with a mix of states; one real-machine check after the release (I do it) |
| CF-CLO-R6 | The session's top bar PR status matches the real state (merged / closed / CI result), and is refreshed after a merge | Component test + real-browser screenshot |
| CF-CLO-R7 | Interrupted cleanup (network failure, app exit) leaves no half-finished state; it resumes and completes next time | Fault-injection test |

### Applicable Harnesses
Spec Harness; Compatibility Harness (existing workspace / delivery record data);
Observation Harness (cleanup results are traceable); Viewport Harness (session
result card + top bar, light/dark); AI Collaboration Harness.

### Test matrix
- Normal: CodeFactory merges it itself → closeout; external merge via the GitHub
  web UI → closeout.
- Edge: an extra unpushed commit after merging; uncommitted files; the remote
  branch was already deleted; network outage during closeout.
- PR closed without merging.
- Historical leftovers: a mix of merged-and-contained / merged-with-extras /
  closed / still open.
- Interrupted: app exits halfway through cleanup → resumes after restart.

### Implementation Notes
- The closeout authority for a workspace is the `delivery_runs` canonical PR
  record (`canonical_pr_number` / `canonical_pr_url` / `canonical_head_sha`)
  cross-checked against the workspace identity. Cleanup starts only when that
  authority and the workspace identity agree exactly.
- When no committed local `provider_pr_merge` receipt exists — the external-merge
  case — the cleanup pass asks a read-only `CanonicalPrObserver` for the
  canonical PR's real state. A `Merged` answer is recorded as a
  `reconciled_committed` `provider_pr_merge` receipt and the ordinary removal
  path then runs unchanged; `ClosedUnmerged` keeps everything and reports it in
  plain language.
- The offline cleanup pass uses `OfflineCanonicalPrObserver` (always `Unknown`,
  i.e. preserve), so the local-only smoke and unit paths keep their exact
  previous behaviour. The background supervisor uses the remote-backed observer
  (GitHub via the logged-in `gh` CLI) and is safe to leave `Unknown` when no
  remote is resolvable: nothing is deleted without positive merge proof.
- Cleanup is idempotent and lease-guarded: a pass that stops after the worktree
  removal (app exit, network failure) resumes at the next pass and only the
  exact branch / exact directory is ever removed.
