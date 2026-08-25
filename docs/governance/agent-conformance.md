# Agent conformance — making the harness binding across different agents

> Design doc for the cross-agent governance-conformance mechanism. Itself a
> "major change" (governance machinery), so per
> `docs/principles/design-docs-for-major-changes.md` it lands as a doc.

## Problem

Different agents touch the same repos — Claude Code, Codex, others — plus
humans. Each reads a *different* convention file (Claude: `CLAUDE.md` + memory;
Codex: `AGENTS.md`). Prose conventions are **advisory**: an agent may not read
them, may interpret them differently, or may simply not follow them. So
"write it in AGENTS.md" does **not** guarantee conformance.

We need a way to make the harness **binding regardless of which agent (or human)
produced a change.**

## Core insight

**You cannot enforce at the agent. You enforce at the choke point every change
passes through: the repo's CI / PR gate.** That gate is agent-agnostic — it
runs identically whether the PR came from Claude, Codex, or a person. Docs
*guide*; the gate *enforces*.

## The three layers

```
①  Canonical rules        docs/principles/*  +  docs/governance/rules.yml
        │  (single source of truth — written ONCE)
        ▼
②  Agent convention files  AGENTS.md (Codex) · CLAUDE.md/memory (Claude) · …
        │  each POINTS to ①, never re-states it (no drift)   [SOFT: guidance]
        ▼
③  CI conformance gate     trusted policy + scenario gate + validators [HARD: blocks]
           required status check on the branch → "not green, not merged"
```

- **① is the only place a rule is authored.** Everything else references it.
- **② makes each agent *likely* to comply** by pointing it at ①. Soft.
- **③ makes *every* change *provably* comply.** Hard. This is the layer that
  actually guarantees cross-agent consistency.

①② without ③ is an honor system. ③ is the load-bearing layer.

For scenario governance, the hard layer has one stable required context:
`scenario-gate-pr`. It is itself a `pull_request_target` workflow that loads
the runner from the default branch and validates the candidate tree without
credentials, including the candidate registry and target bindings. The
protected ruleset requires it with strict latest-base enforcement. This
prevents Codex, Claude, an IDE, or a plain Git client from choosing a weaker
path, and prevents a PR from weakening its own validator to self-attest.

## The machine-readable manifest: `docs/governance/rules.yml`

A single, executable source of truth. Each rule:

```yaml
rules:
  - id: release-cadence
    statement: "Merge continuously, release deliberately; releases are not triggered by merges."
    doc: docs/principles/release-cadence.md
    enforcement: structural          # structural | check | review
    enforcer: .github/workflows/auto-release.yml   # artifact that makes it true
  - id: design-doc-for-major
    statement: "Every major change lands a design doc in the tree."
    doc: docs/principles/design-docs-for-major-changes.md
    enforcement: check
    enforcer: tools/governance/check_governance_rules.py
    level: warn                      # warn now → error later
```

`enforcement` kinds:
- **structural** — the rule is true by construction (e.g. the release workflow
  has no merge trigger; no agent *can* re-enable per-merge releases).
- **check** — a validator actively tests each change for the rule.
- **review** — human/agent review, last resort (kept explicit so we know which
  rules are *not* yet machine-enforced).

## The validator: `tools/governance/check_governance_rules.py`

Run by `governance-baseline.yml` on every PR/push. It:

1. **Loads and shape-validates `rules.yml`** — malformed manifest → blocker.
2. **Verifies each rule's `doc` and `enforcer` exist** — a rule can't claim an
   enforcer that isn't there. This catches *governance drift*: add a principle
   without wiring its enforcement and CI fails. (This is what keeps ① and ③ in
   sync, agent-agnostically.)
3. **Runs `enforcement: check` rules.** For `design-doc-for-major`: inspect the
   PR's changed files (vs the base) with a "major" heuristic (new
   subsystem/module, migration/schema, contract change, or a large source
   diff); if major and the same change adds no `docs/{specs,design,principles}`
   doc and carries no `Design-Doc:` commit trailer → emit per the rule's
   `level` (warn → annotation; error → blocker).

Output matches the existing validator contract (structured failures, non-zero
exit on blockers) so it slots into `governance-baseline`.

## Making it a hard gate

- Add `check_governance_rules.py` as a step in `governance-baseline.yml`.
- In branch protection, mark **`governance-baseline`** (and CI) as **required
  status checks** for `main`. Then a non-conforming PR — from any agent —
  literally cannot merge. This is the step that converts "should" into "must".

## Adoption path (warn → hard)

1. Land manifest + validator with `design-doc-for-major` at `level: warn`.
2. Let a few PRs flow; tune the "major" heuristic to fit reality.
3. Flip to `level: error` and require the check in branch protection.

Structural rules (release-cadence) are already hard from day one — they need no
ramp because there's nothing for an agent to violate.

## Portability

`rules.yml` + the validator + this doc are repo-agnostic. A new repo adopts the
harness by copying them, pointing its `AGENTS.md`/`CLAUDE.md` at `rules.yml`,
and marking the gate required. Same rules, same enforcement, every repo, every
agent.

Local hooks are intentionally only a fast feedback mirror. The canonical
command is:

```text
python tools/governance/run_scenario_harness_gate.py --stage local --repo . --policy-repo .
```

Skipping or not installing the hook does not bypass the protected server-side
contexts. A repository owner can still alter the GitHub ruleset itself; an
organization required workflow or independent GitHub App is the external
trust-root option when owner credentials must also be constrained.
