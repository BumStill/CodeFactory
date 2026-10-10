# CodeFactory can cut a release on its own (U28)

### Background and user decision
Continuous merging, deliberate releases: a release bundles everything merged since the last tag, and may be cut on request (`workflow_dispatch`) or on schedule (see `docs/principles/release-cadence.md`). A release task has no code changes of its own; it should go through a controlled release path, not be forced into "deliver my own changes".

### Decision
CodeFactory provides a controlled release-only path: no branch, no PR; it follows the existing release-cadence rules to check and dispatch the release, watches it until it's published, and keeps verifiable records.

### Requirements Traceability

| Req ID | Requirement | Minimum evidence |
| --- | --- | --- |
| CF-REL-R1 | A session that's asked to release can trigger a release of main with no changes of its own; it doesn't push branches or open PRs | Integration test: zero changes → release triggered, no branch/PR created |
| CF-REL-R2 | Before releasing, follow release-cadence: no hold in the batch, a feat/fix is present (or it's explicitly forced and the user approved that), and the release is bound to the exact main head at decision time. If any of these isn't met, tell the user why plainly and don't release | Table-driven tests: hold / no feat-fix / head moved / normal |
| CF-REL-R3 | After triggering, wait until it's really published (release is no longer a draft, macOS/Windows installers and the update file are present), then report version, included PRs, and the run link. A transient network failure retries the same step and doesn't trigger a second release | Test: retrying after a network failure doesn't release twice |
| CF-REL-R4 | Within 5 minutes of the trigger, confirm the release run really appeared on GitHub; if it didn't, say so plainly and don't keep waiting | Test |
| CF-REL-R5 | Within the same authorization, the safety thresholds don't drop: no force-bypassing a hold, no deleting or rewriting tags/releases, no changing workflows | Counter-example tests |

### Applicable Harnesses
Spec Harness; Release Harness; Observation Harness (release records are traceable); AI Collaboration Harness.

### Test matrix
- Normal: one immediate fix → release published.
- Refusals: hold present; only chore/docs; main moved after triggering.
- Network: EOF/SSL failures during trigger and while watching → retry without a duplicate release.
- Run doesn't appear: no run 5 minutes after triggering → report plainly.
