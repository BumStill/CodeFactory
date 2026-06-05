# Principle: every major change lands a design doc

> **Status:** highest-order principle — applies to **every** repository and
> every contributor, human or agent (Claude, Codex, …). Repo-agnostic; copy
> verbatim. Companion to `release-cadence.md`.

## The principle

**A major change does not land without a design doc in the repo.**

The doc is written **before or alongside** the change (not after), and lives in
the tree (`docs/specs/`, `docs/design/`, or `docs/principles/`), so any
collaborator — human or agent, now or later — can recover *why* and *how*
without reverse-engineering the diff.

## Why

Multiple agents and people work the same repos. Without a written, shared design
the same subsystem gets re-derived three different ways, contracts drift, and
context evaporates between sessions. A landed design doc is the **shared source
of intent** that keeps collaborative development consistent.

## What counts as "major"

Any one of:

- A **new subsystem / module** or a new long-lived capability.
- A **schema / migration** or persisted-format change.
- A change to a **public contract**: a tool/command signature, an API,
  an event, a file format, a permission/policy model.
- A **cross-cutting refactor** (touches many modules or a core path).
- A change to **release / CI / governance** machinery.
- Anything a reviewer would reasonably ask "where's the design?" about.

Small, local, behavior-preserving changes (a bug fix, a copy tweak, a contained
refactor) do **not** need one — don't bureaucratize the trivial.

## Rules (normative)

1. **Doc-first.** For a major change, the design doc lands **before or in the
   same PR** as the implementation. Never "we'll document it later."
2. **In the tree.** It lives under `docs/` (`specs/` for long-lived capability
   contracts, `design/` for a specific change, `principles/` for cross-cutting
   rules) — discoverable, versioned, reviewable. Not in a chat, not in an issue
   only.
3. **Implementation-ready.** It states: problem, scope (in/out), design,
   data-model/contract changes, integration points, tasks, acceptance criteria,
   risks. Enough that **a different agent could implement it** (see
   `docs/self-evolution/P1-*.md` as a worked example).
4. **Linked.** The PR references the doc; the doc is reachable from an index
   (`AGENTS.md` / `docs/specs/`).
5. **Enforced, not trusted.** Conformance is a CI gate (`governance-baseline`),
   not an honor system — see `docs/governance/agent-conformance.md`.

## How to apply it in a new repo

- Keep a `docs/design/` (or `docs/specs/`) directory.
- Declare this rule in `docs/governance/rules.yml` and reference it from every
  agent-convention file (`AGENTS.md`, `CLAUDE.md`).
- Wire the conformance check into the repo's required CI gate so a major PR
  without a design doc is flagged (warn → then hard-fail).
