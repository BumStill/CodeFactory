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
  **(Resolved as of Slice 4.8 — see the Slice 4 update below: this duplicate
  loop is deleted, not the crate itself.)**

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

### Slice 2 — headless `ExecCtx` / tools without `AppHandle` ✅ (mostly pre-satisfied)
**Finding (scouted 2026-07-23):** the groundwork was already in place. `ExecCtx.app`
is `Option` and, under `#[cfg(not(test))]`, exactly ONE tool reads it —
`delegate_tasks`, which spawns UI-session subagents (`app.state::<AppState>()` +
`SchedulerHandles`) and legitimately cannot be headless; it already degrades with
`"delegate_tasks is unavailable in this runtime"`. Every other tool
(read/write/edit/glob/grep/bash/pptx/docx/xlsx/kb/skills/delivery) runs with
`app: None`. So `tools::dispatch` is already headless-runnable.

**What this slice ships:** a regression-guard contract test
(`tools::headless_contract_tests`) that runs the core surface end-to-end
(write→read→grep→bash) through an app-less `ExecCtx` — turning the implicit
property into a guaranteed one so a future tool cannot silently break headless
capability.

**Deliberately NOT touched:** the `#[cfg(not(test))]` gates on `ExecCtx.app` and
`delegate_tasks::execute`. They exist because an `AppHandle`-owning struct linked
into the unit-test EXE reintroduces the Windows `STATUS_ENTRYPOINT_NOT_FOUND`
loader failure (hotfix #166). Removing them is risky and unnecessary here; the
headless runner is a `not(test)` binary that sets `app: None`. The real remaining
friction those gates represent dissolves in **Slice 3**, when `AgentLoop` itself
stops requiring a concrete `AppHandle`.

### Slice 3 — headless `AgentLoop::new_headless` ✅ (shipped)
The seam that makes the real loop constructible with no `AppHandle`.

**What shipped:**
- `AgentLoop.app` is now `Option<AppHandle>`; every remaining `AppHandle` use is
  guarded — usage pings, hooks (`HookRunner::disabled_headless`), skills
  (`enabled_user_skill_prompts` / `prompts_from_skill_dir`, user-skills only),
  and `ExecCtx.app`. The emit path already went through `EventSink` (slice 1),
  so `run()` itself needs no `app`.
- `AgentLoop::new_headless(events: Arc<dyn EventSink>, …, mode)` — same fields as
  `new_with_mode` minus `app` (set to `None`), events supplied by the caller
  (a `CollectingEventSink` for eval, a JSONL sink for a CLI). It constructs no
  `AppHandle` and calls no `AppHandle` method.
- `--headless-smoke <receipt.json>` (`run_headless_smoke_cli`) — a release/CI
  entry mirroring `--evolution-smoke`. It builds the real loop headless on the
  **packaged binary** and asserts the full tool surface is reachable and the
  event sink records. Wired into Windows CI, this proves the loader path #166
  made fragile stays sound on the exact executable — safely, because it's a
  `not(test)` binary, never the unit-test EXE.

**Verification:** full Rust suite green (app-less skills helper + headless
`HookRunner` unit tests run on every platform incl. Windows); `--headless-smoke`
returns `{ok, tool_count:24, events_recorded:1, app_handle:"none"}` and exits 0.

**Deferred to slice 4 (deliberately):** the live headless *turn* (calling `run()`
end-to-end against a model / recorded fixture). Constructing a full `AgentLoop`
inside a `#[cfg(test)]` unit test is avoided — that is the #166 trigger; the
binary smoke is the right vehicle instead.

### Slice 4 — retire `agent-headless`'s duplicate loop ✅ (shipped, re-scoped)
> **Status update (2026-07-28):** this slice's original framing — "delete the
> shell-only crate" — turned out to be the wrong target once the work was
> actually scoped. See `docs/design/sidecar-shared-loop-4.8.md` (slices
> 4.8/4.8a–4.8e) for the full analysis and ground truth; summary below.

What actually shipped: `agent-headless`'s **354-line duplicate copy of the
agent loop is deleted** — "There is one loop." — via a ~180-line adapter
(`run()` in `main.rs`) that builds `LoopInputs`/`RunConfig`/`LoopServices` and
calls the shared `agent-loop::run::run_agent_loop`, the exact same function
`run_openai`/`run_anthropic` use (slices 4.6/4.6b/4.7).

The crate itself is **not** being deleted, and that's correct, not deferred
debt: it's the real CLI binary that speaks the JSONL stdin/stdout protocol a
headless eval runner needs (no `AppHandle`, no GUI), so a thin process
wrapper over the shared loop is the right end state — the same shape as
`run_openai`/`run_anthropic` being thin wrappers inside the desktop binary.
Its line count (3444 across `main.rs` + `protocol.rs`/`transport.rs`/
`compaction.rs`/`policy.rs`/`loop_services.rs`, up from the pre-4.8a 2759)
is legitimate: `main.rs` itself is ~340 lines of production code plus a
deliberately preserved ~1890-line, 28-test regression suite pinning the
eval-scoring-critical behavior (compaction semantics, wall-clock reserves,
budget-denial wording) that 4.8's own analysis flagged as easy to silently
corrupt. Splitting protocol/transport/compaction/policy into their own
modules (4.8a) is the module-boundary cleanup, not new duplication.

**Still genuinely open:** 4.8e — re-baseline with a real, `official_comparable`
Terminal-Bench 21 run (before/after, same trial count) to prove the flip
didn't regress eval scores. (4.8d, a pre-flip differential harness, was
explicitly voided in the 4.8 doc once the flip already shipped — nothing to
resurrect there.)

### Slice 5 — the objective quality signal (the second half of the keystone)
With the full loop runnable headless, wire it into the eval infra: run a task
through `run_headless`, score with the existing verifier substrate, emit a
per-task quality number (not just pass/fail) — the signal P1 optimizes toward and
P4 routes on. This is the payoff that makes P1-optimize and P4-routing non-theatre.
**Not started** — the 4.8 work above was a prerequisite (getting the eval sidecar
onto the one shared loop) rather than this slice itself.

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
