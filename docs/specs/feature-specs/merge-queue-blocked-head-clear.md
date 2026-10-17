# Feature spec: Merge queue — a blocked PR is re-evaluated after a new head

## Background and user decision

On 2026-10-17 PR #693 had a merge conflict and the queue labelled it
`merge-queue: blocked`. A conflict fix was pushed as new head `bba7f4fa`, and CI
went 10/10 green, but every queue run still reported
`not-eligible: already blocked by the queue` for 20 minutes. Nothing could make
progress until a human removed the label by hand.

The queue must therefore remember **which head it blocked**, and treat "the head
moved" as "this is a new subject, evaluate it again".

## Decision

The queue records the blocked head SHA at the moment it blocks a PR. On each
later pass a blocked PR is evaluated against that record:

- head unchanged → stay blocked (the same failing head is never retried in a
  loop);
- head changed → remove the block and re-evaluate normally;
- a legacy block with no record → unblock only when the head's latest commit is
  strictly newer than the label's added time; otherwise stay blocked.

`merge-queue: armed`, `merge-queue: hold`, the required-check set and every gate
are unchanged.

## Requirements Traceability

| Req ID | Requirement | Minimum evidence |
| --- | --- | --- |
| M101-R1 | When the queue blocks a PR it records the blocked head SHA (label description / PR comment / queue state — pick the most robust one and state why) | Test: `markBlocked` writes a machine-readable head marker on the PR; adapter reads it back |
| M101-R2 | On a later pass: PR head ≠ recorded SHA → remove the blocked label automatically and re-evaluate through the normal path; head == recorded SHA → keep the block (never retry the same failed head) | Tests: "new head clears the block and the PR merges"; "same head stays blocked and is not retried" |
| M101-R3 | No record found (older label) → conservative: unblock only when the head's latest commit time is later than the label's added time | Test: "legacy label without a record"; both the newer-head and older-head directions, plus missing timestamps stays blocked |
| M101-R4 | Tests in `scripts/merge-queue.test.mjs` cover the three cases above, and are seen failing first | Test run output before/after the fix |
| M101-R5 | No change to `armed` / `hold` semantics, to the required checks, and no gate is weakened | Existing R1/R3/R5 tests stay green; ruleset guard untouched |

## Applicable Harnesses

Spec Harness; Compatibility Harness (merge-queue / label / workflow contract is
used by the delivery tool and the release bump PR); Observation Harness (each
unblock is a traceable queue outcome); AI Collaboration Harness. Governance
surface: `scripts/merge-queue.mjs` is part of the trusted root, so the
orchestrator handles the `scenario-gate-pr` cannot-self-modify rule through the
ruleset process — this change does not touch any gate logic.

## Test matrix

- Normal: blocked head superseded → label removed once → PR merges.
- Repeat: blocked head unchanged → still `not-eligible`, zero merge attempts,
  no duplicate blocking comment.
- Legacy: label without a record, head committed after the label → unblock;
  head committed before the label → stay blocked; timestamps missing → stay
  blocked (conservative).
- Regression: armed/hold/ruleset-guard behaviour unchanged.

---

## Implementation Notes (does not change any requirement)

### Why the record lives in a PR comment

- A label description is repository-global: one `merge-queue: blocked` label is
  shared by every PR, so it cannot hold a per-PR SHA.
- The queue is stateless (schedule / `workflow_run` / `labeled` runs on trusted
  default-branch code with no persistent store), so a local state file would be
  lost between runs.
- A PR comment is per-PR, survives runner restarts, and is readable by humans
  and by `gh api repos/{repo}/issues/{n}/comments`. The queue writes a hidden
  HTML marker followed by the human-readable reason:

  `<!-- merge-queue:blocked head=<sha> at=<iso> -->`

- Reading is lazy: comments (and, only when the marker is absent, the issue
  events for the label time plus the head commit time) are fetched **only** for
  PRs that carry a blocked label, so a normal queue pass makes no extra calls.
