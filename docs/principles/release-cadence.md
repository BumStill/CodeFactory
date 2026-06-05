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
4. **One release = one coherent batch.** The version bump is computed from *all*
   commits since the last tag (highest wins: any `feat!`/breaking → major, any
   `feat` → minor, else any `fix` → patch). The changelog aggregates the batch.
5. **Never release red or unverified.** Don't cut on top of a failing main; the
   build must be green and artifacts complete before publish, and the "latest"
   pointer must resolve to the **highest** version (guard against
   out-of-order/rerun publishes stealing "latest").
6. **The pipeline is resilient.** Transient infra failures (push 500s, registry
   download drops) retry rather than abort. See the repo's release workflows.

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
