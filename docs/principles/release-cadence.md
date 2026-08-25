# Principle: Merge continuously, release deliberately

> **Status:** highest-order principle — applies to **every** repository (this
> one and any future one), and to every contributor, human or agent (including
> Codex). This file is intentionally repo-agnostic so it can be copied verbatim
> into any project.

## The principle

**Integration and delivery are separate concerns.**

- **Merge continuously.** Land small, verified changes onto the main branch as
  often as they're ready. CI runs on every merge for fast feedback.
- **Release deliberately.** A *release* (a tagged version with built artifacts
  shipped to users) is a **deliberate, batched act** — never an automatic
  side effect of a merge.

Merging ≠ releasing. Ten merges can become one release.

## Why

Auto-releasing on every merge produces:

- **Update fatigue** — users are prompted to update many times a day.
- **Wasted build cost** — a full multi-platform build per trivial change.
- **Version churn** — `v1.27 … v1.35` in an afternoon, where the numbers carry
  no meaning.
- **Noisy changelogs** — one line per version instead of a coherent set.

Batching fixes all four, and you lose nothing: you can still release in minutes
— you just *choose* to.

## Adaptive cadence: severity and size choose the speed

"Release deliberately" is not "release slowly". A batching rule that says *when
not to release* but never says *when to release now* collapses into one speed:
whatever the schedule happens to be. A P0 crash fix then rides the same bus as a
typo fix and reaches users up to a full cycle late.

So the decision is **adaptive** — driven by the severity of the problem and the
size of the change — with a routine target and an anti-churn ceiling:

- **Routine maximum wait.** When gates and release infrastructure are healthy,
  the scheduled cut means user-facing work should go out within one scheduled
  cycle (this repo: 24 hours). A red gate, explicit hold, or infrastructure
  failure is a visible blocker, not a reason to pretend the time target was met.
- **Ceiling (the anti-churn rule).** A schedule tick with no user-facing change
  produces **no release**. "At least one release per day" means *at most 24 hours
  of waiting*, never *a version number per day*. Forcing out a `chore`/`docs`-only
  version re-creates the update fatigue and meaningless version numbers this
  principle exists to prevent.
- **Express lane.** A change that meets the urgency rubric below is cut
  **immediately after merge**, without waiting for the schedule.

### The urgency signal is metadata, not prose

A rule that says "release severe things faster" is interpreted differently by
every contributor and every agent, and cannot be audited afterwards. Encode it in
the commit instead, as a trailer:

```
Release-Urgency: immediate
```

| Value | Meaning | Effect |
| --- | --- | --- |
| `immediate` | Meets the rubric below | Requires the merge operator to dispatch right after merge, even when the ordinary delivery boundary would wait |
| *(absent — the default)* | No urgency override | Follows the repository's configured boundary: an already-authorized on-demand `through_release` still dispatches; otherwise it rides the schedule |
| `hold` | Must not ship alone (needs a companion change, docs, or an announcement first) | Blocks every cut until the whole batch is reviewed and deliberately released with `allow_guarded_batch=true` |

A `hold` remains active for the whole unreleased batch. Commits are immutable,
so a later commit cannot silently remove or supersede the trailer. Once the
dependency, documentation, or announcement is ready, a maintainer reviews
everything since the last tag and manually dispatches Auto Release with
`allow_guarded_batch=true`. The ordinary `force` input only permits a release
without a `feat`/`fix`; it deliberately cannot bypass a hold. The resulting tag
clears the held range. Do not rewrite published history merely to remove a hold.

A commit trailer — not a PR label — because it travels in `git log`, works in any
repository and any tool, needs no forge-specific setup, and the release workflow
is already reading `git log <last-tag>..HEAD`. Do not overload Conventional
Commits' `!`; that already means "breaking change" and drives the version slot.

The trailer is a **decision signal, not an event trigger**. Rule 1 still forbids
release-on-merge, so `immediate` requires the already-authorized merge operator
to dispatch Auto Release after the merge. It does not replace a deliberately
configured on-demand path: CodeFactory's `deliver_changes` with a
`through_release` ceiling is itself an authorized dispatch request, even without
an urgency override. For squash merges, the operator must preserve the trailer
in the final squash commit body; a trailer that exists only on a discarded
branch commit cannot be audited from `main`. `BREAKING CHANGE` /
`BREAKING-CHANGE` and every `Release-Urgency` line must remain in one continuous
final footer block. CodeFactory's GitHub adapters pass that final subject/body
explicitly and verify the merged commit retained all of this release metadata
before proceeding.

### Rubric for `immediate` (any one is sufficient)

1. The primary user path is unusable, or the product fails to start or crashes.
2. Data is lost, corrupted, or persisted incorrectly.
3. A security, credential, or permission boundary is bypassed.
4. A **released** version is exposing the defect to users right now (a regression
   they can already feel).
5. The user explicitly said it is urgent.
6. A large, self-contained capability just landed complete: holding it only delays
   value without reducing risk.

**When in doubt, use the default.** `immediate` inflating into "every merge" turns
this back into release-on-merge and forfeits every benefit above. The rubric is a
short list of severe conditions, not a mood.

### A release carries other people's work

Cutting a version ships **everything merged since the last tag**, not just the
change that triggered it. So before an `immediate` cut, scan
`git log <last-tag>..HEAD`: if any commit in the batch carries
`Release-Urgency: hold`, do **not** cut automatically. Wait until its dependency,
documentation, or announcement is ready, review the complete batch, then use the
explicit `allow_guarded_batch=true` dispatch. This is what makes `hold`
load-bearing rather than decorative. An unrecognised `Release-Urgency` value is
guarded the same way, so a typo cannot silently bypass the stop signal.

### Who may decide

An agent inside an already-approved task may apply the rubric and cut the release
itself: `PR -> CI -> merge -> release -> artifact verification` is one authorization
chain, not four. Proposing a release with no task in flight still needs the user's
go-ahead.

## Rules (normative)

1. **CI on every merge; release on neither push nor merge.** The release
   pipeline MUST NOT be triggered by a push to main.
2. **Releases are cut on demand or on a schedule.** Trigger via a manual
   "cut release" action (`workflow_dispatch`) and/or a low-frequency schedule
   (e.g. once daily). Both batch *everything merged since the last tag* into one
   release.
3. **Only user-facing changes ship.** A release happens only when there is at
   least one `feat:` or `fix:` (Conventional Commits) since the last tag.
   `chore` / `ci` / `docs` / `refactor` / `test` / `style` changes **do not, on
   their own, cut a release** — they ride along in the next feat/fix release.
   (A manual `force` escape hatch may override this for an exceptional cut.)
   This rule is **not** weakened by the waiting-time target: the schedule
   bounds how long a releasable change waits, it never manufactures a release
   out of non-user-facing commits.
4. **Severity and size choose the speed.** With healthy gates, the scheduled
   cut is the routine maximum wait, not the only exit. A change meeting the
   urgency rubric carries `Release-Urgency: immediate`, requiring an on-demand
   dispatch even when ordinary policy would wait. A separately configured
   on-demand `through_release` boundary remains deliberate and may dispatch
   without an urgency override. A change that must not ship alone carries
   `Release-Urgency: hold` and blocks every cut while it is in the batch. A
   guarded batch can only be released by a deliberate
   `allow_guarded_batch=true` dispatch after review.
   See "Adaptive cadence" above for the rubric and the batch-scan obligation.
5. **One release = one coherent batch.** The version bump is computed from *all*
   commits since the last tag (highest wins: any `feat!`/breaking → major, any
   `feat` → minor, else any `fix` → patch). The changelog aggregates the batch.
6. **Never release red or unverified.** Don't cut on top of a failing main; the
   build must be green and artifacts complete before publish, and the "latest"
   pointer must resolve to the **highest** version (guard against
   out-of-order/rerun publishes stealing "latest").
7. **Put heavyweight verification before publishing.** Full local/project tests,
   type checks, builds, governance checks, and primary-path acceptance belong
   before merge/release. After a release is published, verification should be
   limited to release facts: the intended commit is contained in the tag, the
   release is not a draft, required assets are present and downloadable, the
   updater/latest pointer resolves to that version, and any configured live smoke
   proves the shipped artifact/service is reachable. Do **not** rerun full test
   suites after publish unless the release workflow changed code after the
   pre-release gate, the pre-release evidence is missing/stale, or a release/live
   smoke actually failed and needs repair.
8. **The pipeline is resilient.** Transient infra failures (push 500s, registry
   download drops) retry rather than abort. See the repo's release workflows.
9. **Failed unpublished tags are immutable tombstones, not retry loops.** If a
   release run failed and main has no releasable change after that tag, bounded
   recovery may retry the same source. Once main contains a `feat`/`fix`, the
   controller must preserve the failed tag for audit and cut the next version;
   it must never move or rebuild the old tag while claiming the fix is present.
   Release impact is always measured from the previous **published** release,
   so a tombstone cannot hide unshipped product changes from scenario gates or
   release notes.

## How to apply it in a new repo

- Set the release/version workflow's trigger to `workflow_dispatch` (+ optional
  `schedule`), **not** `push` / `workflow_run-on-every-CI`.
- Compute the bump from `git log <last-tag>..HEAD`, classifying by Conventional
  Commits; **skip the release entirely if no `feat`/`fix` is present** (unless
  forced).
- Keep CI on every push for feedback.
- Reference this principle from `AGENTS.md` so agents (Codex included) honour it.

## Reference implementation

`.github/workflows/auto-release.yml` in this repo implements all of the above
(dispatch + daily schedule, batch slot from commits-since-tag, skip-when-no
feat/fix, CI-green guard, retry-hardened push). Copy it as a starting point.
