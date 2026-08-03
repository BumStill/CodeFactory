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

The ceiling is also an explicit on-demand release policy. The optional
`release_urgency` tool field adds repository cadence metadata without silently
raising that user-owned boundary:

- `immediate` writes `Release-Urgency: immediate` so merge/release operators
  must take the express lane.
- `hold` writes `Release-Urgency: hold`, allows verified integration through
  merge, then blocks `trigger_release` until the complete batch is reviewed and
  manually dispatched with `allow_guarded_batch=true`.

### Hybrid provider — GitHub, GitLab, and hookable enterprise remotes

Local ops (stage / commit / push) shell out to the `git` CLI, exactly like
`commands/git.rs` and `checkpoint.rs` — no new runtime dependency. Remote ops
(PR / MR / CI / merge / release) go through the `DeliveryRemote` trait.
Built-ins are:

- GitHub: logged-in `gh` CLI first for GitHub checkouts, then configured
  token+REST via `git_remotes`.
- GitLab / enterprise GitLab: configured `git_remotes` token opens or reuses a
  GitLab merge request; merge uses GitLab's MR merge endpoint.
- `delivery_provider` hook: repositories can register a Hook (`event =
  "delivery_provider"`, `RunCommand`) whose command receives JSON on stdin and
  returns JSON on stdout for `open_or_get_pr`, `ci_status`, `merge_pr`, and
  `trigger_release`. This is the plugin seam for private GitLab variants or
  custom enterprise forge/release systems.

A non-GitHub origin must never be reported as “no GitHub channel”. Missing
credentials are provider-aware: GitLab asks for a GitLab remote token or a
`delivery_provider` hook/plugin.

### Noise-safe staging — the structural guarantee

Delivery NEVER runs `git add -A`/`git add .`. `stage_scoped` runs `git add -u`
(stages tracked modifications/deletions, adds no untracked file) then adds only
untracked `??` entries that pass the built-in noise denylist (`.claude/`,
`CLAUDE.md`, `AGENTS.md`, `src-tauri/gen/schemas/`, `codex-worktrees/`,
`.codefactory/attachments/`, …) plus the user's `delivery_exclude_globs`. Local
junk can never be swept into a delivery commit.

### Verification phase boundary — heavy before publish, light after

Delivery distinguishes quality gates from release-fact checks. Heavyweight
verification — project tests, type checks, builds, governance checks, and
primary-path acceptance — belongs before merge and release. Once GitHub (or any
forge/release system) has published an immutable tag and release assets, the
post-release step should not rerun the whole project suite by default. It should
only confirm the facts users can observe: the PR/MR is merged, the release tag
contains the intended merge commit, the release is no longer a draft, required
assets exist and are downloadable, the latest/updater pointer resolves to that
version, and any configured deployment/live smoke passes.

Escalate back to targeted heavy verification only when pre-release evidence is
missing or stale, the release workflow generates/modifies code or packaging
logic after the gate, or a release/live smoke fails. Rerunning full suites after
publish as a routine completion ritual is intentionally disallowed: it cannot
change the already-published artifact and it wastes user-visible delivery time.

### Idempotent / resumable state machine

`deliver()` records the requested ceiling separately from the effective ceiling,
then walks the safely achievable steps. Reaching a reduced effective ceiling is
a recoverable `blocked` outcome, not `delivered`: the result names the missing
capability, actual reached state, and one continuation action.

Each step checks reality first:
nothing-to-commit is a clean skip, an already-open PR is reused via
`list_prs(head)` (never double-opened), CI-red/conflict/no-token are **blocked**
terminals with a clear message — never a loop, never a double-apply. Re-invoking
after a crash continues from the real git/PR state. Before merge and release
dispatch, delivery writes a repo-local `intent_merge`/`intent_release` receipt;
a confirmed response upgrades it to `merged`/`release_triggered`. The receipt
key and body are bound to schema version, credential-free canonical remote
identity, remote name, base, head, and commit SHA. This prevents different
branches or repositories that share a tip from overwriting each other's
idempotency state. Same-context, same-tip retries reuse completed receipts and
only rerun missing observation. Corrupt, unknown, unreadable, mismatched, or
lingering intent receipts fail closed; an ambiguous external result is
structurally non-retryable until the remote fact is inspected.

The receipt starts at `pr_open`, before CI or merge. It preserves the PR title
and final release-metadata body for that exact branch tip, so a later
parameterless call can resume after `PrOnly`, pending CI, or an app restart
without replacing the original `BREAKING CHANGE` / `Release-Urgency` policy
with a generic PR body.

For GitHub squash merges, delivery supplies the final commit subject/body rather
than relying on forge defaults. If the branch or PR contains release metadata,
delivery keeps `BREAKING CHANGE` / `BREAKING-CHANGE` and `Release-Urgency`
trailers in one final footer block. Both the CLI and REST adapters then read the
remote PR title/body again immediately before merge, rebuild the policy from
that source of truth, and finally read the merged commit back to verify that
every such trailer survived before the state machine may proceed to release.
Major-version intent, `hold`, and unknown urgency values are therefore
load-bearing gates, not branch metadata that squash can silently discard.

The structured `DeliveryOutcome` truth fields are also carried as tool metadata
through the desktop backend, stream event, frontend tool-call state, and the
normalized `tool_calls.metadata` column. Retryability and reached state never
depend on parsing the localized report body.

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
- **GitHub rulesets and merge queue**: delivery reads the effective required
  checks for the PR base branch, waits until every required context is present
  and terminal, then submits auto-merge bound to the observed head SHA. A
  ruleset rejection remains a real blocker; it is never re-labelled as a local
  high-risk-command denial and is never bypassed with `--admin`.
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
