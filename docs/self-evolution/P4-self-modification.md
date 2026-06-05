# P4 — Self-modification (design + the safe contract)

> Phase 4, the boldest: **the factory improves its own code.** CodeFactory is
> already AI-built; P4 internalizes that loop. It is also the highest-risk phase
> — so this doc leads with the safety contract, then ships only the safe
> foundation. The autonomous code-writing part is **deliberately gated**, not
> auto-built.

## The non-negotiable contract

A self-change to CodeFactory's own codebase follows, in order, and never skips a
step:

```
detect friction → draft a PROPOSAL → (implement on a branch) → VERIFY → open a PR → HUMAN APPROVES → merge
                       ▲                                                                   │
                       └────────────────────── the system never crosses this line ────────┘
```

- **The system never applies code to itself.** No auto-commit to main, no
  auto-merge, no auto-release of a self-change. Ever.
- **A human approves every self-change** at the PR gate (the `governance-baseline`
  required check + review). This is the same agent-agnostic gate from
  `agent-conformance.md` — a self-authored PR is treated exactly like any other.
- **Verification before the PR** (the `verify` skill): a self-change must be run
  and observed, not just compiled.
- **Reversible**: a self-change is a PR (revertable), never a push; checkpoints
  cover the rest.
- **Bounded + audited**: rate/scope caps; every proposal cites the friction
  (evidence) that motivated it.

This is why P4 is last: it inherits *all* of the safety model
(`README.md`) at maximum strength.

## v1 (this PR): the safe foundation — a self-improvement proposal

The only part safe to ship + run unattended: **read-only analysis that produces
a proposal for a human.** It writes no code, opens no PR, changes nothing.

`self_improvement_proposal()` aggregates friction **globally** (across all
projects) — reusing P1's deterministic detectors (tool reliability, retry-prone
failures) — and renders a markdown **改进提案** that:

- lists the top recurring friction points with their evidence counts, and
- for each, names the *kind* of fix to consider (e.g. "add a pre-check in the
  tool's implementation") — a hint, not a patch.

It explicitly states, in its own header, that it modifies nothing and that any
action is the human's. The proposal-rendering is a pure, unit-tested function.

## Deliberately deferred (NOT auto-built while unattended)

The autonomous **draft → branch → implement → verify → PR** pipeline for
self-changes. Building a system that writes code to its own repo — even
proposal-gated — is exactly the kind of change that must itself be designed,
reviewed, and enabled by a human, behind:

- branch-protection required checks (governance-baseline + CI), so a
  self-authored PR cannot merge without human approval;
- an explicit, per-capability opt-in (a setting the human turns on);
- a hard rate/scope cap.

Shipping that pipeline silently, while the user is away, would violate the very
safety model this phase is supposed to embody. So v1 stops at the proposal; the
next slice (the gated implementer) lands as its own reviewed PR when the human
chooses to enable it.

## Acceptance criteria (v1)

- `self_improvement_proposal` reads only (no writes, no PR, no release) and
  returns a markdown proposal whose header makes the human-gate explicit.
- The proposal-rendering is deterministic + unit-tested (empty, and
  with friction).
- It reuses P1's detectors (no duplicate friction logic) and queries globally.
- It never hard-fails (best-effort, like the rest of the learning module).
