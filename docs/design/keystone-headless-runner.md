# Keystone — Headless Product-Agent Runner

The one large project the adversarial review chartered (see
`product-capability-adversarial-review-2026-07.md`): decouple the real agent
loop from `AppHandle` so the SAME loop — with the FULL tool surface — can run
without the Tauri desktop shell, and can be observed/evaluated from outside the
GUI. It simultaneously unlocks P5's real suite, P1's efficacy measurement, and
P4's per-model quality data.

## Ground truth (scouted 2026-07-23, against `main`)

- `AgentLoop` (`src-tauri/src/agent/mod.rs`) holds `app: AppHandle`. Its coupling
  is overwhelmingly **one thing**: `self.app.emit(event_name, StreamEvent)` — the
  UI progress stream (~40 call sites, uniform shape).
- The only other `AppHandle` uses are ~6 `self.app.clone()` handed to: tool
  `ExecCtx.app` (already `Option<AppHandle>`), `anthropic_client` (an
  `app_handle: &AppHandle` param that emits directly, 5 sites), and hooks.
- **There are TWO agent loops.** `agent-headless` is a **separate 2759-line
  reimplementation** exposing `run_shell` ONLY, with **zero** reuse of
  `crate::tools` (grep = 0). This fork is the waste the keystone removes: the
  headless path cannot reach `read_xlsx` / `kb_search` / `deliver_changes`.

**Decision: unify, do not grow the fork.** Extract an event-sink abstraction and
make the real `AgentLoop` runnable headless with the full tools; retire the
shell-only fork once parity is proven.

## Slices (each an independent, verifiable PR — no big-bang)

### Slice 1 — `EventSink` trait (load-bearing, mechanical, zero behavior change)
Introduce `trait EventSink: Send + Sync { fn emit(&self, event: StreamEvent); }`.
- `TauriEventSink { app, event_name }` implements it as exactly
  `app.emit(&event_name, event)` — the UI stream is byte-identical.
- `AgentLoop` gains `events: Arc<dyn EventSink>`; replace every
  `self.app.emit(event_name, StreamEvent::…)` with `self.events.emit(…)`.
- `AppHandle` STAYS on the struct for now (tools/hooks/anthropic still use it);
  only the emit path is abstracted.
- Also route `anthropic_client`'s 5 emits and `emit_transport_retry` through an
  `&dyn EventSink` param instead of `&AppHandle`.
- **Verify:** all existing tests green; a `CollectingEventSink` (Vec of events)
  unit-tests that the loop emits the expected sequence for a scripted turn — the
  first time the loop's event output is testable at all.

### Slice 2 — headless `ExecCtx` / tools without `AppHandle`
`ExecCtx.app` is already `Option`. Audit each tool: which genuinely need a UI
channel (only the interactive secret prompt from `configure_git_remote`, which
is reverted). Make the full `tools::dispatch` runnable with `app: None` +
`EventSink`. Interactive-only tools degrade to a clear "not available headless"
error (already the pattern).

### Slice 3 — headless `AgentLoop::run_headless`
A constructor/entry that builds an `AgentLoop` with a `CollectingEventSink`
(or a streaming JSONL sink), `app: None`, an in-memory or file-backed history,
no Tauri. Runs the SAME loop, SAME gate, SAME tools. Returns the final transcript
+ event trace.

### Slice 4 — retire `agent-headless` fork / repoint its callers
Once `run_headless` reaches parity, delete the 2759-line shell-only crate (or
reduce it to a thin CLI over `run_headless`). This is where the duplication
finally dies.

### Slice 5 — the objective quality signal (the second half of the keystone)
With the full loop runnable headless, wire it into the eval infra: run a task
through `run_headless`, score with the existing verifier substrate, emit a
per-task quality number (not just pass/fail) — the signal P1 optimizes toward and
P4 routes on. This is the payoff that makes P1-optimize and P4-routing non-theatre.

## Non-goals / guardrails
- Zero UI behavior change through Slice 1–2 (pure refactor; the whole point is
  that the desktop app is unaffected).
- No new model provider, no routing, no computer-control — those wait on the
  signal this produces.
- Each slice ships behind its own PR + CI + release; never merge a half-migrated
  emit surface.

## First build: Slice 1
Start here. It is the smallest change that makes the loop's observability
testable and is the prerequisite for every later slice.
