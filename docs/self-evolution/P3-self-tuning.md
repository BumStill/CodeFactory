# P3 — Self-tuning (design + v1 scope)

> Phase 3 of self-evolution (see `README.md`). Where P0–P2 adapt *content*
> (memory, skills), P3 adapts *behavior parameters* from outcomes. This doc
> defines the full surface and ships one safe, concrete slice.

## The idea

The agent's behavior has knobs that are currently fixed or hand-set:

- **The proposer's own bias** — how readily the post-mortem proposes a `memory`
  vs `preference` learning.
- **Dispatch routing** — plan-first vs execute-first per model (today tuned
  internally; could learn from re-ask/approval outcomes).
- **Per-project tool policy** — which tools to auto-allow vs gate (today a
  static default; P1 already detects flaky tools).
- **Prompt phrasing / compliance booster** — strength per model.

P3 closes a feedback loop on these: read outcomes → nudge the knob →
better outcomes. **All nudges stay bounded, surfaced, and reversible** — see the
safety model in `README.md`. P3 never silently changes a security-relevant
setting (e.g. it may *suggest* gating a flaky tool, but enabling that gate is a
human decision, like P2 skills).

## v1 scope (this PR): proposer self-calibration

The smallest safe, high-signal slice — and the one P1's detector-4 already
measures: **feed the user's accept/reject history per learning kind back into
the post-mortem prompt.** If the user has rejected most `preference` proposals,
tell the proposer to only offer a `preference` when highly confident; if they
accept most `memory` proposals, say those are welcome.

- Pure, unit-tested `calibration_hint(decisions) -> String`.
- Injected into `run_postmortem`'s prompt next to the A3 "already known" block.
- Bounded: only fires per kind with `>= 4` decisions at an extreme accept-rate
  (`<=25%` or `>=80%`); produces at most a couple of advisory lines.
- Reversible + transparent: it only shapes *what the proposer suggests*, which
  the user still reviews and accepts/rejects (the P0 gate). It changes no
  setting and writes nothing.

This makes the proposer measurably better over time without any new surface:
the more you curate learnings, the better its proposals fit you.

## Later slices

| Knob | Signal | Adaptation | Gate |
|---|---|---|---|
| Dispatch routing | re-ask / approval outcomes per model | bias plan-vs-execute | structural / internal |
| **Tool policy** ✅ | P1 tool-reliability | *suggest* gating a flaky tool (allow→ask) — **shipped**, see `P3-tool-policy.md` | **human enables** (like P2) |
| Compliance booster | post-approval re-asks per model | strengthen/relax | internal |

(Dispatch routing + compliance booster remain designed, not yet built.)

Each later slice gets its own PR; security-relevant ones (tool policy) are
proposal-only — the human applies them, never the system.

## Acceptance criteria (v1)

- `calibration_hint` is deterministic and unit-tested across reject-heavy,
  accept-heavy, below-threshold, and empty cases.
- The hint appears in the post-mortem prompt only at the defined extremes.
- It changes no persisted setting and never blocks the post-mortem (best-effort,
  like the rest of `run_postmortem`).
