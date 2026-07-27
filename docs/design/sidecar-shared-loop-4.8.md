# Slice 4.8 — point the eval sidecar at `run_agent_loop` (SCOPED, NOT YET EXECUTED)

Deep analysis of `crates/agent-headless/src/main.rs` (2775 L) against
`agent-loop::run::run_agent_loop`. **Conclusion: 4.8 is materially different from
4.6/4.6b/4.7 — it is not a mechanical wire-up, and a single-PR flip is unsafe.**
Recorded here so the work is properly scoped rather than rushed.

## Why this one is different
4.6/4.6b/4.7 were *behaviour-preserving* refactors provable by unit tests. 4.8
changes what the eval harness sends to the model, so its real acceptance bar is a
**Terminal-Bench 21 comparable run** (CF-TB-R29, same trial count,
`official_comparable: yes`) — "all 27 tokio tests green" is necessary but nowhere
near sufficient. Getting this wrong silently corrupts the objective quality
numbers keystone exists to produce.

## ⚠️ The verified silent-failure trap
`policy::completion_command_and_kind` computes
`let kind = if tool_name == "bash" { classify_command(…) } else { ToolKind::ReadOnly }`.
**The sidecar's tool is named `run_shell`** (main.rs:749). A naive port therefore
classifies EVERY sidecar tool call as `ReadOnly` ⇒ the completion gate never sees
a `Mutation` ⇒ total, silent gate failure. **No agent-loop unit test catches
this.** Any 4.8 attempt must fix `ToolOutcome` construction FIRST (item b2).

## Divergence ledger
**(a) Expressible today — no core change.** RunConfig (`gate_benchmark:true`,
`progress_window:4`, `recovery_limit:1`, `Benchmark`, `max_iterations:max_steps`);
`SidecarTransport` (buffered POST, retries, timeout clamp, required→auto fallback,
`run_shell` schema, sanitize+clear on finalization, usage accumulation);
`DelegatingToolBackend` (JSONL round-trip over `Arc<Mutex<stdin/stdout>>`, strict
id correlation); `NullPersistence`/`NoOpHooks`/`NoOpFactChecker`; a
`JsonlEventSink` that swallows desktop `StreamEvent`s; `RuntimePolicy` as a
`PermissionGateway`.

**(b) Needs NEW seams in `run_agent_loop` — 15 items, 5 of which touch the
desktop path** (so they risk #135/#136 + StreamEvent byte-identity as collateral):
`ContextCompactor` seam (b1); **`ToolOutcome` from the real `ToolInvocationResult`
instead of the `(content, is_error)` synthesis (b2 — the trap above; also
desktop-visible)**; wall-time-aware budget denial (b3); pluggable denial wording
(b4); inspection-budget rule needing `ProgressTracker` state at the denial point
(b5); mutable working directory (b6); actually wiring `Budget::may_continue`
(b7 — currently destructured as `_budget`); mid-batch wall-reserve abort (b8);
graceful stop on transport error in the final reserve (b9); `RunOutcome`
enrichment — `final_text`/full `Usage`/stop reason (b10); time-convergence prompt
(b11); rejected-draft replay (b12); `usage_snapshot`-on-error and
`usage_snapshot`-when-no-tool_request hooks (b13/b14);
`model_request_attempts(tool_history.len())` (b15).

**(c) Unavoidable eval-scoring changes.** `Value → ChatMessage` can never be
byte-identical: `content:null → ""`, added `"type":"function"` and
`"name":"run_shell"`, dropped provider-extra fields, reordered keys. That shifts
the 40 000-char compaction trigger to a different round even with a perfect
compactor. Plus `classify_command`'s timeout argument differs (wall-clamped vs a
fixed 300 000 ms), changing `ToolKind` for bounded probes.

## Compaction: the core incompatibility
Sidecar `compact_messages` is **char-based** (40 000 chars of serialized JSON),
single-pass, **destructive** (everything between index 2 and the last
tool-calling assistant is discarded forever), replaced by a ≤30-entry digest of
untruncated tool history, and it **mutates the loop's history in place**.
`compress_if_needed` is **token-based** (75 % of the window), elides in place with
previews, then drops oldest user-turns. These are not variants of one another.
`context_compression:false` ⇒ unbounded history ⇒ context-window 400s ⇒
catastrophic. `true` ⇒ different history ⇒ guaranteed score change. Only a
`ContextCompactor` seam preserves the semantics — and even then (c) remains.

## 4.8c progress (shipped)
- **b7** — already landed by #204 (`Budget::may_continue` is wired at the
  segment-continue boundary). Not duplicated.
- **b10 ✅ v1.64.18** — `RunOutcome` carries accumulated tokens, the last model
  reply, and a `StopReason`.
- **b1 ✅ v1.64.18** — `ContextCompactor` seam; desktop supplies
  `DefaultCompressor` (byte-identical), sidecar can plug its char-budget digest.
- **b2 ✅ v1.64.18** — the loop feeds the gate from the backend's real
  `ToolInvocationResult`; `DesktopToolBackend` classifies with the same rule as
  before. Closes the `run_shell`→always-`ReadOnly` silent gate failure
  *post*-execution.
- **b3/b4 ✅ merged** — `Budget::wall_time()` owns the clock and feeds
  `evaluate_budget_command_with_time_in_directory` (identical for desktop, which
  passes `None`); `BudgetDenial{rule,reason}` is structured and worded by
  `PermissionGateway::format_budget_denial` (defaulted to today's sentence).

### ⚠️ b5 blocked on a PRE-execution twin of the b2 trap
The inspection-budget rule needs the tool's `ToolKind` *before* the call runs, to
decide `read_only_exhausted() && kind == ReadOnly`. The only pre-execution
classifier today is `policy::completion_command_and_kind`, which still has the
`tool_name == "bash"` gate — so for the sidecar's `run_shell` **every** call
would classify `ReadOnly` and the rule would fire on everything (b2 fixed only
the *post*-execution path, which reads the backend's result).

**Fix first: add a defaulted `ToolBackend::classify(&self, call, args) ->
(String, ToolKind)`** (default = today's `completion_command_and_kind`), have the
loop use `tools.classify(...)` for the budget denial, and only then add the
inspection rule behind a `RunConfig` flag. The backend already owns
post-execution classification after b2; this makes it own pre-execution too,
which is the coherent end state.

### The flip's real remaining blocker (corrected)
The sidecar's six seam impls now exist and compile (`agent-headless/src/loop_services.rs`),
so `run()` is the only thing still on the old body. Working the two
`usage_snapshot` gaps through properly:

- **b13 (transport error) needs NO loop hook.** `SidecarTransport` holds the same
  shared `Arc<Jsonl>` stdout as the tool backend, so it can emit the snapshot
  itself immediately before returning a `TransportError`. Entirely internal to
  the sidecar's transport impl.
- **b14 (a round that emitted no `tool_request`) needs ONE defaulted method:**
  `EventSink::round_ended(&self)`, called once per model round after the tool
  batch. The sidecar's sink emits a snapshot only when no `tool_request` went out
  that round (a flag it shares with the backend). Desktop sinks keep the no-op —
  the same shape as the existing `usage_recorded`.

The bridge invariant this protects: **every model round emits at least one line
carrying usage** (`tool_request` OR `usage_snapshot`); `codefactory_bench/agent.py`
depends on it via `_latest_usage_snapshot`.

So the remaining flip is: add `EventSink::round_ended` → rewrite `run()` as an
adapter → adjust the 28 tokio tests for the new emission timing → 4.8d
differential harness → 4.8e re-baseline (needs a real TB-21 run).

## Recommended decomposition (each independently green)
1. **4.8a ✅ SHIPPED** (PR #205) — sidecar internal module split
   (protocol/transport/compaction/policy), zero behaviour change, the 28-test
   tokio suite byte-for-byte unchanged (596 production lines moved out, 12
   `mod`/`use` lines in). Diffable baseline for everything below.
2. **4.8b — RE-SCOPED after attempting it: fold into 4.8c.** The "adopt the
   traits behind its own loop" idea works for the desktop (whose loop is already
   shared) but NOT here — the sidecar's parts only become detachable once the
   loop is shared. Verified in code:
   - `PermissionGateway` — **blocked**. The decision reads
     `progress_tracker.read_only_exhausted()`/`mutation_seen()`, `wall_time`,
     `gate.evidence()` and `cwd`; the trait only passes
     `(tool_call, args, bash_command)`. Needs seams b3/b4/b5 first.
   - `SidecarTransport` — **imposes (c1) immediately**: today's loop echoes the
     provider `message` Value verbatim, so returning a typed `ModelResponse`
     changes the payload before anything else does.
   - `NullPersistence` — **ceremony**: the sidecar never persists.
   - `JsonlEventSink` — **low value**: its outputs (`tool_request`/
     `usage_snapshot`/`finished`) are not `StreamEvent`s.
   - `WallClockBudget` — clean, but a single small rule on its own.
   The one piece with real forward value is **`DelegatingToolBackend`** (the
   JSONL tool round-trip behind `execute()`), and it requires converting
   `&mut stdin/stdout` to `Arc<Mutex<…>>` through all of `run()` — an I/O
   ownership restructure of a governance-tracked binary whose failure modes
   (deadlock, reordered output lines) the 28 tests may not catch. Do it as its
   own focused change, not as a tail-end step.
3. **4.8c** — add each `run_agent_loop` seam separately, verified against the
   DESKTOP suite. **b2 and b5 deserve their own PRs.**
4. **4.8d** — differential harness before the flip: run both loops against the
   same canned `fake_openai_server` scripts and diff the emitted JSONL **and** the
   captured outbound payloads. Merge only when the JSONL diff is empty and the
   payload diff is exactly the accepted (c) set.
5. **4.8e** — re-baseline: an `official_comparable: yes` Terminal-Bench 21 run
   before and after, same trial count. Any pass-rate regression is a blocker.

## If schedule pressure forces one PR
Do **4.8a + 4.8b only** and leave the sidecar on its own loop body. That retires
the transport/tool/output duplication (the bulk of what the refactor targets)
while leaving the eval-scoring surface — compaction, gate feeding, denial policy,
wall-clock control — untouched.
