# Product Capability — Adversarial Review (2026-07)

Consolidated output of an adversarial multi-agent review (21 agents: grounded
proposal → 3 independent red-team lenses per problem → cross-problem synthesis)
of five core product problems raised by the founder. Grounded against the
codebase; each verdict survived feasibility / root-cause / over-engineering
attacks. This is the durable plan — edit it here as reality changes.

> **Framing note.** The review was run against a worktree pinned at v1.51.7.
> Much of the "reactive first-aid" it recommends as *parallel work* had already
> shipped to `main` across v1.55–v1.61 (vision degradation, context-overflow
> compression, provider-overload backoff, `turn_error` persistence, gh-first
> delivery, onboarding wizard, self-recovery contract + fact-check registry).
> That is independent validation, not contradiction: a from-scratch adversarial
> analysis reached the same "reactive failure handler is the highest-ROI fix"
> conclusion. What remains genuinely unbuilt is the **keystone** and its cheap
> forerunner **P5-lite**.

## The single root cause (the uncomfortable truth)

Four of the five problems (P1, P4, P5, and P2's "screenshot-as-completion-proof"
tendency) are symptoms of **one** missing thing:

> **CodeFactory has no model-independent "behavioral quality" signal, and no
> substrate (a headless product-agent runner decoupled from `AppHandle`) that
> can produce one.**

- The completion gate is a *verification-ordering checker* — it proves "some
  mutation was verified" (its own comment, `agent/mod.rs`), **not** a quality
  oracle.
- Therefore: **P1 cannot optimize** (no objective to optimize), **P4 cannot
  route** (no per-model quality signal to rank on), **P5 cannot evaluate the
  real tool surface** (the only headless runner is shell-only), and P2 is
  tempted to treat a self-scored green-check screenshot as proof.

The one big project worth chartering is: **a headless product-agent runner
(decouple `agent/mod.rs` from `AppHandle`) + an objective quality signal.** It
simultaneously unlocks P5's real suite, P1's efficacy measurement, and P4's
per-model quality data.

## Per-problem verdicts (build / cut)

### P1 — best-effort · find-the-optimum · consolidate learning
Diagnosis accurate: the loop has **TRY**, lacks **OPTIMIZE**, and **CONSOLIDATE**
is complete plumbing but off-by-default / human-gated / quality-blind / prose-only
(`learning_events` → `improvement_candidate` → eval → `evolution_active_memory`
→ injected via `build_learnings_section`).

- **BUILD:** extend the existing coverage-audit gate (`build_completion_ready_prompt`,
  already ~80% there) from Autonomous-only to Execute (`completion_ready_applies`,
  fix the two mode asserts) + add a "simpler/cheaper re-verifiable alternative"
  clause + **a failable oracle**: force it to write and run a test with an input
  NOT in the request. Only then does "re-pass the gate" mean anything.
- **CUT:** a new 4th `CompletionFinalization::Optimize` enum arm + `optimize_limit`
  + new call sites (gold-plating a mechanism that already exists). **CUT** the
  "local-model, on-by-default consolidation pass" — *disproven*: there is no
  local model (`resolve_postmortem_model` calls the cloud `default_endpoint`), so
  on-by-default = exfiltrating every session's trajectory. Keep the human-review
  promotion gate; land agent-authored skills DISABLED as today.

### P2 — computer control
- **BUILD (minimal, prove the need first):** `ToolOutput.image: Option<…>` (Default,
  zero-regression) + a read-only `screenshot` behind ONE bool, registered only on
  vision-capable routes. But a minimal screenshot must honestly solve one of:
  all THREE `ApiStyle`s wired (Anthropic/OpenAI/**Chatgpt** — the Codex/gpt-5
  route the founder actually runs), OR image blocks persisted with the DB Message
  (so `repair_incomplete_tool_history` / turn-replay don't drop them), OR image
  budget/eviction (the Anthropic path has no elision compression — a screenshot
  loop blows the window). Cheapest first probe: bash → headless-chromium screenshot
  → return via the **existing** `attachments.rs` `file://` image channel.
- **CUT:** the desktop-input automation stack (enigo, `classify_computer_action`,
  guarded per-action HITL, phase-3 VM). 4 of the proposal's own 6 risks are all on
  the input side; it violates the user's own "No GUI interference / idle ≥ 300s"
  rule and deadlocks autonomous (no HITL).

### P3 — built-in-ness / ease of use
The real wall: the packaged default endpoint ships **without a key**, so the user
must obtain and paste a third-party secret before the first message.

- **BUILD (re-ordered by value):** (1) make **ChatGPT keyless OAuth the onboarding
  default** — the single change that makes the app "built-in" at message one;
  (2) demote the provider-preset idea to a ~15-line TS constant dropdown (must
  include a localhost/LMStudio line) with free-text "Custom"; (3) fix the
  add-remote dead-end **at the failure point** (inline "add remote", prefill from
  origin), not via onboarding surgery; (4) render the already-complete
  `src/stores/mcp.ts` CRUD store (an afternoon), don't ship a read-only tab.
- **CUT:** the Rust `ProviderPreset` registry + command; `GITHUB_TOKEN`/`gh` env
  scavenging (empty on GUI launch; reverses the "never assumes gh" contract).

### P4 — auto model routing + cross-model consistency
Over-engineered. None of the three field failures is a routing failure; the
single-endpoint default user never triggers `decide_route`; active
`reasoning_effort`/`thinking` injection is exactly the v1.19.2 regression already
reverted.

- **BUILD (reactive core only):** generalize the existing HTTP-400 self-heal
  (`force_max_completion_tokens`) into `classify(status, body) -> Transient |
  ContextOverflow | VisionUnsupported | FieldUnsupported | AuthOrQuota | Fatal`
  at the one call site, replacing the bare `?`: VisionUnsupported → strip images
  + retry once; ContextOverflow → compress + retry once; else → a persisted,
  visible `turn_error`. Plus a name-based `supports_vision` pre-strip. One site,
  one small function, three unit tests — fixes all three failures for every user
  including single-endpoint. *(Largely SHIPPED v1.55–v1.70 as separate fixes.)*
- **CUT:** RoutingPolicy DSL, `decide_route`, cross-endpoint failover, the
  `ModelProfile` mega-struct, coupling settings edits to evolution-eval, and any
  "absorb/beautify" refactor of working code. Defer routing until there exist
  ≥2 interchangeable same-api endpoints AND a quality/cost signal (from P5).

### P5 — evaluation mechanism
Direction right (extend `benchmark.rs`, don't touch evolution safety gates, don't
rewrite), but the flagship deliverable was built on the wrong execution substrate:
the only independently-callable headless core is **shell-only** (`run_shell`), so
"run office-xlsx through the sidecar" is a category error (it tests "can it write
shell"). `read_xlsx`/`kb_search`/`deliver_changes` live only in the AppHandle-bound
in-process `AgentLoop`.

- **BUILD NOW (days — this is P5-lite, the recommended first move):**
  (a) a **read-only cross-model consistency + failure-distribution report** over
  existing `benchmark_trials` rows (pass-set Jaccard, reward spread, completion-gate
  divergence, divergent-task list), honoring the R29 comparability gate;
  (b) `task_type` / `sweep_id` / `evaluation_axis` columns via `ensure_column`;
  (c) a static **capability-coverage matrix** for P1 (tool exists? has eval? y/n);
  (d) deterministic **tool-correctness** integration tests that call `tools::dispatch`
  directly for Office/kb/delivery — real product tool surface, no model, no Harbor.
- **CUT / DEFER:** `codefactory-capability-v1` over headless, per-task verifier
  authoring + contamination-scanner expansion, P2 state-change subset. The full
  agentic suite waits on an explicit go/no-go on funding the headless runner.

## Sequencing

```
[keystone, weeks–months] headless product-agent runner (decouple agent/mod.rs from AppHandle) + objective quality signal
        │  ← P5 real suite, P1 efficacy measurement, P4 per-model quality data all depend on it
        ▼
P5-lite (failure distribution + cross-model report over existing rows, DAYS)  ← decides whether P1/P2/P4 each merit their reinvestment
        │
        ├─► P4 reactive failure handler (one call site)  — same site as P2 vision degradation; also stops turns dying   [mostly SHIPPED]
        ├─► P1 OPTIMIZE (needs a failable oracle + P5 efficacy measurement)
        └─► P4 routing (needs P5's per-model quality/cost signal)

P3 (ChatGPT-first onboarding) — independent, cheap, high user value, parallel anytime   [onboarding wizard SHIPPED v1.61]
```

**Highest-leverage first step: P5-lite.** It is days of cost and the only thing
that turns the founder's five *hypotheses* into *evidence*. Three independent
red teams each demanded "mine the real failure distribution before investing
weeks." Without this ruler, P1's optimize loop and P4's routing engine are built
on guesses.

## Do NOT build (over-engineering traps the red team exposed)

1. Per-candidate benchmark A/B + auto-activate (violates frozen-dataset contract;
   its only client, auto-activation, is not a requirement).
2. Local-model, on-by-default consolidation pass (no local model exists).
3. Full desktop-input automation stack (cost all on the non-core input side;
   violates "No GUI interference").
4. RoutingPolicy DSL + `decide_route` + active field injection (reverts v1.19.2;
   never triggers for single-endpoint default users).
5. Rust `ProviderPreset` registry + command, `GITHUB_TOKEN`/`gh` scavenging.
6. Any "absorb/beautify" refactor of working code (regression risk, zero
   user-visible gain).

## First implementation: P5-lite

See `docs/design/p5-lite-consistency-report.md` for the concrete design being
built now — a read-only aggregation over `benchmark_trials`/`benchmark_runs`
producing a cross-model consistency + failure-distribution report, gated on R29
comparability, with the pure aggregation core under exhaustive unit tests.
