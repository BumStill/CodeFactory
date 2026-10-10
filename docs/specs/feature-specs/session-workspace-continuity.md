# Session workspace continuity — U26 + U27

## Background and user decision
The user's principles: continuing in the same session = continuing the same job; the user should not have to know how many internal "tasks" or "workspaces" exist behind the scenes; already-finished work must never be dropped silently.

## Decision
As long as a session still has unfinished work (uncommitted / unpushed / an open PR that isn't merged), a new message in that session continues in the same workspace on the same branch, and delivers to the same PR. A fresh workspace is used only once the work has been merged or the user has explicitly given it up.

## Requirements Traceability

| Req ID | Requirement | Minimum evidence |
| --- | --- | --- |
| CF-WSC-R1 | When the previous task ended in **any** state (completed / failed / stopped) and its workspace still has uncommitted, unpushed, or not-yet-merged PR content, a new message in the same session continues in the same workspace: the agent sees the same changes, the same branch, the same PR | Table-driven tests: completed / failed / stopped × uncommitted / unpushed / open PR |
| CF-WSC-R2 | A workspace with unfinished content is never cleaned up while the session can still continue; only workspaces whose content is fully contained in the merged result, or that the user has explicitly given up, can be cleaned | Tests: cleanup pass skips each kind of unfinished workspace |
| CF-WSC-R3 | A continued task can deliver to the PR the session already has (updating the same PR), with no identity conflict and no new branch or new PR | One synthetic end-to-end: first delivery opens PR → task ends → continue and modify → delivery updates the same PR |
| CF-WSC-R4 | If continuing in place is truly impossible (e.g. the workspace is damaged or missing), the session tells the user in plain words where the changes are and what will happen next; never silently start from an empty workspace | Component/integration test + wording passes the internal-vocabulary guard |
| CF-WSC-R5 | Once the previous work has been merged, a new message gets a fresh workspace based on the latest main (doesn't drag the old branch along) | Test |
| CF-WSC-R6 | After continuing a task that ended in any state (completed / failed / stopped), the agent really starts working again (model calls and tool calls both happen), with no internal error loop; this still holds when the app was restarted between the task ending and the continuation | Synthetic end-to-end: failed → restart → continue → at least one successful tool call |
| CF-WSC-R7 | A delivery record left by a previous attempt doesn't block delivery after continuing: on continuation it's re-reconciled against the current workspace and the remote, never requiring the user to restart the app | Integration test: previous delivery record identity is stale → continue → delivery succeeds |
| CF-WSC-R8 | The same deterministic internal error doesn't burn the recovery budget: once the same failure repeats, stop retrying and tell the user plainly what happened and where their work is | Test: same signature appears twice → stops, plain-language summary (passes the internal-vocabulary guard) |

## Applicable Harnesses
Spec Harness; Compatibility Harness (existing workspace data in the old format must continue correctly); AI Collaboration Harness.

## Test matrix
- Normal: completed + staged changes → continue → same workspace; completed + open PR → continue → same PR.
- Regression: failed / stopped (U24's existing behaviour unchanged).
- Edge: workspace directory deleted by hand; main has advanced meanwhile; continued twice in a row.
- Terminal: after merging, continue → fresh workspace.
- Cleanup: a cleanup pass runs at the same moment as the continue (must not delete it out from under the continuing task).

## Implementation Notes
The spec is delivered by two tasks against the same Requirements Traceability table.

- U27 (`fix(workspace): replace #590 and stabilize Windows continuation test`, #595) implements CF-WSC-R6, CF-WSC-R7 and CF-WSC-R8.
- U26 (this task) implements CF-WSC-R1, CF-WSC-R2, CF-WSC-R3, CF-WSC-R4 and CF-WSC-R5.

U26 scope notes (no requirement is changed by these notes):

- The hand-over introduced by U24 for a *stopped* Objective is widened to every terminal
  Objective state (`completed`, `cancelled`, `failed`) and now keys the carry-forward
  decision on "the work reached a merge", not on "a PR number is recorded somewhere".
  An open, not-yet-merged PR is unfinished work under CF-WSC-R1/R5.
- CF-WSC-R3 is carried by the hand-over keeping the exact worktree, branch and recorded
  canonical PR binding, plus the existing delivery rule that an already-open PR for the
  same head is reused instead of duplicated. U26 adds the test that proves it end to end;
  it does not change the requirement.
- CF-WSC-R4 is surfaced through the managed-workspace status the product already renders
  (`ExecutionWorkspaceView.failure_code` / `failure_detail`, shown in the workspace view),
  and its wording is asserted against `failure_summary::assert_no_internal_vocabulary`.
