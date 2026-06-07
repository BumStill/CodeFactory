# P3 — Tool policy (flaky-tool gating proposals)

> A later slice of P3 self-tuning (see `P3-self-tuning.md` → "Later slices").
> Where P3 v1 tuned the *proposer*, this slice closes the loop on **tool
> reliability**: P1 already mines which tools fail a lot — this turns that
> signal into a safe, human-gated adjustment to the permission policy.

## The loop

```
P1 detects flaky tool  →  propose gating it  →  human clicks 启用  →  tool moves allow→ask  →  agent confirms before running it
   (observe)               (read-only)            (the gate)           (one settings edit)      (existing decide_permission)
```

There is **no new enforcement machinery** — it rides the existing
`decide_permission` policy (`allow` / `ask` / `deny`). Because `allow` is
matched before `ask`, gating a currently-allowed tool just means *removing it
from `allow`* so it falls through to `ask`.

## Why this is the safe slice

- **Safe direction only.** Accepting a proposal makes a tool *more* cautious
  (auto-run → confirm), never more permissive. A wrong gate costs one extra
  confirmation prompt — annoying, not dangerous — and is undone in Settings.
- **Human-gated.** The system *proposes*; nothing changes until the human
  clicks 启用门控. Same preview-then-enable contract as P2 skills.
- **Read-only detection.** `propose_tool_gates` reads `tool_calls` + the
  current policy only; it mutates nothing.
- **Reversible + transparent.** Enabling moves the tool into the visible `ask`
  list in Settings; the user can move it back to `allow` anytime.

## v1 scope (this PR)

- `propose_tool_gates()` — read-only. Runs P1's `detect_tool_reliability`
  globally (≥8 calls, ≥25% failure) and proposes gating only the flaky tools
  that are **currently auto-allowed** (so accepting actually changes behavior).
  `bash` (already asks) and `skill_*` (always allowed; gated when the human
  enables the skill) are never proposed.
- `apply_tool_gate(tool)` — the human-gated enable. Removes `tool` from
  `permissions.allow` and records it under `permissions.ask`, then persists via
  the same path as `save_settings` (disk + in-memory `AppState`). Idempotent;
  only ever tightens.
- A 「工具门控建议」 section in Profile, beside the self-improvement proposal:
  scan → list → 启用门控 per tool.
- Pure `tool_gate_proposals(insights, allow)` is unit-tested.

## Deliberately out of scope

- **Auto-applying a gate.** Always human-clicked — never silently, even for a
  100%-failing tool (the safety model in `README.md`).
- **Loosening policy.** This slice never moves a tool *into* `allow` or grants
  new access; it only proposes the cautious direction.
- **Per-project tool policy.** v1 edits the global policy; per-project gating is
  a later slice.

## Acceptance criteria (v1)

- `tool_gate_proposals` is deterministic + unit-tested: proposes a flaky tool
  that's in `allow`, skips a flaky tool that's already gated, skips `bash`.
- `propose_tool_gates` reads only; `apply_tool_gate` only ever moves a tool
  allow→ask and persists; neither hard-fails.
- The agent's existing `decide_permission` enforces the gate with **no change**
  to the permission layer.
