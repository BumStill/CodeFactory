# Self-Evolution — overview & roadmap

> CodeFactory's tagline is **软件工厂 · 本地助手 · 自进化**. "Self-evolution" is
> the system by which the product gets better from its own use — the factory
> improving the factory. This document is the durable north star: it defines
> the loop, the phased roadmap, and the safety model so the work can be picked
> up by anyone (human or agent, including Codex) without re-deriving the design.

## The loop

Self-evolution is one closed feedback loop, applied at widening scopes:

```
   Observe ─────────► Reflect ─────────► Adapt ─────────► better outcomes
   (what happened)    (what to learn)    (change behavior)        │
        ▲                                                         │
        └─────────────────────────────────────────────────────────┘
```

- **Observe** — capture outcomes: task success/failure, retries, verification
  results, tool errors, cost, user accept/reject of learnings, retrieval hits.
- **Reflect** — extract lessons from those outcomes.
- **Adapt** — turn lessons into changes to behavior, at increasing scope:
  memory → skills → tuning → (ultimately) the product's own code.

## What already exists (the substrate)

| Loop stage | Already in CodeFactory |
|---|---|
| Observe | `task_runs` (status, attempt_count, error, `verification_results`), `learning_events`, `tool_calls` (name/status/error/duration), `cost_entries`, `retrieval_events`, `checkpoints` |
| Reflect | per-session post-mortem (`commands/learning.rs::run_postmortem`) |
| Adapt (memory) | preferences + accepted learnings injected into every chat (A1), under a unified context budget (A2), deduped + conflict-flagged (A3) |

**The memory system (A1–A3) is the first closed turn of this loop** — reflect →
memory → behavior. Everything below widens the "Adapt" scope.

## Roadmap (scope of adaptation grows; risk grows with it)

| Phase | Adapts | Gist | Risk |
|---|---|---|---|
| **P0 — substrate** ✅ | memory/preferences | Memory system A1–A3 (done) | low |
| **P1 — cross-session pattern mining** ✅ | memory quality | **Shipped** (`mine_cross_session_patterns`). Aggregate across many sessions (not one) to surface evidence-backed patterns: "tool X fails often", "decompositions shaped like Y verify-pass", "user always rejects learnings of kind Z". Feeds higher-quality learnings + signals for P2–P3. See `P1-cross-session-pattern-mining.md`. | low |
| **P2 — skill auto-evolution** ✅ | agent capability | **Shipped** (`propose_skills_from_patterns` — writes *disabled* proposal skills the user previews + enables; never auto-enables). A recurring task pattern → auto-propose/refine a skill. Reuses the existing skill system. See `P2-skill-auto-evolution.md`. | medium |
| **P3 — self-tuning** ✅ | routing/policies | **Shipped** (proposer self-calibration: accept/reject history per learning kind feeds back into the post-mortem). Later slices (dispatch routing, tool policy) are designed; security-relevant ones stay human-enabled. See `P3-self-tuning.md`. | medium |
| **P4 — self-modification** | **its own code** | **Foundation shipped** (`self_improvement_proposal` — read-only friction → markdown proposal for a human; writes no code, opens no PR, ships nothing). The autonomous implement→verify→PR loop is **deliberately gated** behind human opt-in + branch-protection, not auto-built. See `P4-self-modification.md`. | high |

## Safety model (non-negotiable for every phase)

Self-evolution without guardrails is how you get drift and damage. Every phase
inherits these:

1. **Human-in-the-loop.** Behavior/code changes require human approval (P4
   always; memory/preference changes use the existing *preview-then-enable*).
   The system **proposes**, the human (or an explicit policy) **disposes**.
2. **Verification gate.** Every self-applied change passes the `verify` skill
   (run it, observe it) and CI before it lands. Nothing ships red. (Example:
   PR #72 was caught by CI before merge — that gate is the point.)
3. **Reversibility.** Every step is undoable — `checkpoints` already exist;
   memory/skills changes are revertable; code changes are PRs, not pushes.
4. **Bounded autonomy + audit.** Rate/scope caps on what the system may change
   per window, and a full audit trail (who/what/why) for every adaptation.
5. **Evidence over vibes.** An adaptation must cite the outcomes that justify it
   (e.g. "支持证据: 7 sessions"), never a single anecdote.

## Where to start

**P1.** It builds directly on the memory system (A1–A3), is low-risk, and its
output (recurring, evidence-backed patterns) is exactly the input that P2
(skill suggestions) and P4 (self-modification) need. P4 is the most ambitious
and must come last, behind the heaviest guardrails.
