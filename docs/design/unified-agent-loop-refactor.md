# Unified Agent Loop — retiring the duplicate loop (keystone slice 4)

> Supersedes the original slice-4 charter in `keystone-headless-runner.md`
> ("delete the 2759-line fork"). That charter was written on an incomplete
> reading. This is the corrected, verified plan.

## What we actually have (scouted 2026-07-23, workflow `wf_496ab4f7-322`)

There are **two** agent loops, and the second is **not waste** — it is a live,
spec-governed, CI-tested evaluation component:

- **Desktop `AgentLoop`** — `src-tauri/src/agent/mod.rs`. `run()` (mod.rs:876) is
  a dispatcher that forks by `api_style` into two near-duplicate loop bodies:
  `run_openai` (mod.rs:958) and `run_anthropic` (mod.rs:2595). Executes tools
  **in-process** via `crate::tools` (`ExecCtx`, mod.rs:1409/1438), emits via
  `EventSink`/`TauriEventSink`, persists to SQLite.
- **Headless sidecar** — `src-tauri/crates/agent-headless/src/main.rs` (2759
  lines, crate `codefactory-agent-headless`). A **stdin/stdout JSONL** binary
  driven by `Harbor → codefactory_bench/agent.py (_run_sidecar) → sidecar`. It
  advertises **only** `run_shell` and **delegates execution back to the Harbor
  container** (emits a `tool_request`, awaits a `tool_result` incl.
  `next_working_directory`). No DB, no tauri.

The sidecar is governed by **CF-TB-R28** (clean-checkout reproducible build of
`codefactory-agent-headless`), the **contract-hash handshake** (CF-TB-R10/R11,
`agent_contracts/execution_completion.md`), the **CF-TB-R49** watchdog
(argv/pgid), 27 in-crate tokio tests, and the `agent-bridge-linux` CI job
(`tests/test_benchmark_integrity.py`, `test_codefactory_bench_agent.py`,
`test_runtime_acceptance.py`, `test_terminal_bench_*`). **Deleting it breaks the
Terminal-Bench 21 eval capability.**

### What already unifies them
The **entire completion gate lives in `codefactory-agent-core`** and is driven
identically by both loops: `CompletionGate`, `CompletionEvidence`,
`ProgressTracker`, `ToolOutcome`/`ToolKind`, `classify_command`, every
`build_*_prompt`, and the pure mode-policy free functions
(`completion_finalization`, `completion_recovery_limit`, …, mod.rs:3310-3669).
The duplication is the **loop mechanics**: model transport, context management,
iteration control, tool dispatch, output — each written twice (desktop×2 provider
bodies, sidecar×1).

## Target architecture

One shared, **tauri-free** loop crate; the surface-specific pieces are injected
as trait objects and live in whichever crate owns their heavy dependency.

```
crates/agent-core     (unchanged: serde/sha2/url pure — the gate vocabulary)
crates/agent-loop     (NEW, codefactory-agent-loop): tauri-free
    ├ types           (moved from src/openrouter/types.rs; re-exported)
    ├ EventSink        (moved from src/agent/events.rs; re-exported)
    ├ traits: ToolBackend, Persistence, ModelTransport, Budget
    ├ run_agent_loop(config, transport, backend, persistence, events, budget)
    └ the pure mode-policy fns (moved from mod.rs), + a FinalizationPolicy enum
codefactory (bin, src-tauri): depends on agent-loop
    ├ TauriEventSink            (owns AppHandle)
    ├ DesktopToolBackend        (owns Option<AppHandle> under #[cfg(not(test))])
    ├ SqlitePersistence         (owns SqlitePool + the `anonymous` flag)
    └ DesktopModelTransport     (openai/chatgpt/anthropic HTTP)
codefactory-agent-headless (sidecar): depends on agent-loop
    ├ DelegatingToolBackend     (JSONL tool_request/await tool_result)
    ├ JsonlEventSink            (usage_snapshot translation)
    ├ NullPersistence + WallClockBudget
    └ SidecarTransport          (non-streaming buffered POST)
```

**#166 discipline (load-bearing):** `agent-loop` **never links tauri**. Every
`AppHandle`-owning type (`DesktopToolBackend`, `SqlitePersistence`,
`TauriEventSink`) stays in the bin crate under the existing `#[cfg(not(test))]`
gates. The loop only ever sees `Arc<dyn ToolBackend>` etc.; the unit-test EXE
constructs only `#[cfg(test)]` **stub** trait objects — never an AppHandle-owning
struct. See [[project-windows-loader-apphandle]].

### The four traits (from the architect synthesis)

```rust
#[async_trait]
pub trait ToolBackend: Send + Sync {
    async fn list_schemas(&self) -> Vec<ToolDefinition>;              // desktop=all+MCP; sidecar=[run_shell]
    async fn execute(&self, call: &ToolCall, ctx: &ToolCtx) -> ToolInvocationResult;
}
// ToolInvocationResult carries {content, is_error, command, kind: ToolKind,
// return_code, stdout, stderr, error, next_working_directory, duration_ms}
// so the loop builds a ToolOutcome + feeds gate/tracker ONCE for both surfaces.

#[async_trait]
pub trait ModelTransport: Send + Sync {
    async fn complete(&self, system_prompt: &str, messages: &[ChatMessage],
                      tools: &[ToolDefinition], opts: &RoundOptions,
                      events: &dyn EventSink) -> Result<ModelResponse, TransportError>;
}
// system_prompt SEPARATE (Anthropic top-level; OpenAI/sidecar fold in).
// required-tool-choice→auto fallback lives ONCE in the wrapper.
// vision-strip / overflow / overload strip-and-retry move INTO desktop complete();
// context compression stays in the LOOP (it mutates history), per-transport flag.

pub trait Persistence: Send + Sync {                                  // write-only
    // persist_message, persist_gate_message(_once), mark_rejected_candidate,
    // record_tool_call_outcome, record_usage_event, persist_cancelled_tool_batch
}
// SqlitePersistence owns `anonymous` and no-ops every write when set — the
// no-DB-trace guarantee moves from ~6 scattered checks into ONE place.
// NullPersistence (sidecar) no-ops everything.

pub trait Budget { /* wall-clock / step ceiling; benchmark-only wall budget */ }
```

**EventSink split (important):** `EventSink`/`StreamEvent` carry streaming +
`usage_snapshot`. The terminal `finished{final_text, execution_contract_sha256,
completion_evidence, usage}` contract is a typed **`RunOutcome` return value**,
NOT an event — the sidecar's `finished` and the contract-hash handshake must not
pollute the desktop `StreamEvent` UI contract. The JsonlEventSink and
DelegatingToolBackend share **one `Arc<Mutex<stdout>>`** so
`usage_snapshot`/`tool_request`/`finished` interleave in the exact pinned order.

## Slice plan — each an independent PR keeping desktop + sidecar + Windows CI green

- **4.1 — crate + wire-type/EventSink code-motion (zero behavior change).**
  Create `crates/agent-loop`. MOVE `src/openrouter/types.rs` (276 lines, already
  serde-only/tauri-free) → `agent-loop::types`; re-export via
  `pub use codefactory_agent_loop::types::*;` so every `crate::openrouter::types::*`
  path keeps compiling. MOVE `EventSink` (+ `CollectingEventSink`) → agent-loop;
  leave `TauriEventSink` in bin. **Green:** desktop builds + byte-identical
  StreamEvent; sidecar untouched; Windows CI unaffected (no new tauri linkage).
- **4.2 — trait definitions only.** Define `ToolBackend`/`Persistence`/`Budget`/
  `ModelTransport` + `ToolCtx`/`ToolInvocationResult`/`RoundOptions`/`ModelResponse`/
  `RunConfig`/`RunOutcome` in agent-loop, with `#[cfg(test)]` `StubBackend`/
  `NullPersistence` + object-safety tests. Nothing consumes them yet.
- **4.3 — desktop `ToolBackend`.** `DesktopToolBackend` in bin
  (`#[cfg(not(test))]` app field). Route BOTH duplicated tool-exec seams
  (mod.rs:1427-1439 + the run_anthropic dup) through `execute()`; build
  `ToolOutcome` uniformly, collapsing `record_completion_outcome` + the sidecar's
  inline construction. **Green:** `headless_contract_tests` (Unknown-tool
  sentinel, MCP-first), `run_headless_smoke`, #166 stub.
- **4.4 — desktop `Persistence`.** `SqlitePersistence` (owns pool + anonymous;
  no-ops when anonymous). Delete scattered `if self.anonymous`. Hoist the ChatGPT
  `reasoning_effort` DB read (mod.rs:1828) out of the transport into a
  pre-resolved `RoundOptions` field so transport is DB-pure. **Green:** anonymous
  no-write tests; #135/#136 gate-message persistence.
- **4.5 — desktop `ModelTransport` (OpenAI + ChatGPT).** Move vision/overflow/
  overload reactive retries into `complete()`; required→auto fallback once in the
  wrapper. Anthropic still on the old path. **Green:** OpenAI/ChatGPT byte-identical.
- **4.6 — shared loop body (openai-style).** Extract `run_openai` into
  `agent-loop::run_agent_loop(...)`; move the pure mode-policy fns in,
  parameterized by a `FinalizationPolicy`. Desktop `run()` becomes a thin adapter
  for openai/chatgpt. **Green:** full desktop gate/finalization suite
  (mod.rs:4758-5900) incl. `exhausted_recovery_releases_with_warning`.
- **4.7 — Anthropic canonicalization (the one non-transparent change).** Convert
  `run_anthropic` to canonical `ChatMessage` + edge-conversion inside the
  Anthropic transport (system_prompt separate, `Value` at the edge, streaming via
  injected sink), compression flag OFF; route through `run_agent_loop`; delete the
  second loop body. **Green:** Anthropic byte-identical (no compression events, no
  added TransportRetry, overload backoff preserved).
- **4.8 — sidecar onto the shared loop.** `DelegatingToolBackend` +
  `JsonlEventSink` + `NullPersistence` + `WallClockBudget` + `SidecarTransport`
  (non-streaming). Sidecar `run()` calls `run_agent_loop` with a **Benchmark**
  `RunConfig` (gate `benchmark=true`, `ProgressTracker(4)`, Benchmark
  finalization arm, wall budget); `main()` writes `finished` from `RunOutcome` +
  contract hash. `main()`/argv/process-group/**binary name UNCHANGED**. **Green:**
  ALL sidecar tokio tests (`protocol_output_uses_exact_bridge_schema`, usage
  timing, tool_choice, wall-reserve, inspection-budget, semantic-failure),
  contract-hash handshake, `agent-bridge-linux`, CF-TB-R28/R49.
- **4.9 — cleanup (optional).** Reconcile duplicate `classify_command`
  (shell_policy vs agent-core) and duplicate contract-hash impls; remove dead code.

## Invariants every slice must preserve (must-not-break)

1. **JSONL wire byte-exactness** — `Start`/`ToolResult` in, `tool_request`/
   `usage_snapshot`/`finished` out: field names, ordering, emission timing
   (pinned by `protocol_output_uses_exact_bridge_schema`, main.rs:1065 + Python
   bridge). Renaming a field silently breaks the out-of-repo Harbor adapter.
2. **Contract-hash handshake** — sidecar rejects `Start` on
   `execution_contract_sha256` mismatch (main.rs:594); bridge rejects `finished`
   on mismatch (agent.py:362). One shared `execution_completion.md`; the hash
   moves wholesale, never forks (CF-TB-R10/R11/B7).
3. **Binary/build/launch (CF-TB-R28, CF-TB-R49)** — crate/binary name
   `codefactory-agent-headless`, `cargo build -p codefactory-agent-headless`,
   `--codefactory-runtime-token=<hex>` argv marker, `start_new_session` pgid
   leadership. `main()` spawns identically.
4. **#166 Windows loader** — agent-loop stays tauri-free; AppHandle/pool owners
   stay in bin under `#[cfg(not(test))]`; unit-test EXE builds only stub trait
   objects. `--headless-smoke` gate stays green.
5. **Completion finalization (#135/#136)** — Interactive/Execute release-with-
   warning; only Autonomous Error/Blocked; recovery limits 3/3/1 (mod.rs:4758-5900).
   The new Benchmark arm must not perturb these.
6. **Divergent constants as explicit config** — gate `benchmark` false/true,
   `ProgressTracker` 8/4, `require_action`, wall-budget present only in benchmark,
   network policy only in benchmark.
7. **MCP-first / native-fallback precedence + `Unknown tool` sentinel**
   (tools/mod.rs:136); `headless_contract_tests` + `run_headless_smoke` green.
8. **Anonymous no-DB-trace** — nothing written when anonymous (now central in
   SqlitePersistence).
9. **Desktop StreamEvent byte-identical** (TauriEventSink unchanged); no
   TransportRetry events added to the Anthropic path.
10. **Both CI suites green** — windows `check` cargo test + `agent-bridge-linux`
    (harbor==0.15.0).

## Hardest problems (ordered)
1. Anthropic canonicalization (Value↔ChatMessage, separate system prompt, streams
   from inside transport, skips compression) — migrate LAST, its own slice.
2. Finalization policy is structurally 4-way (desktop) vs 2-way (sidecar); the
   Benchmark arm must reproduce the sidecar branch byte-for-byte.
3. Shared-writer ordering (one `Arc<Mutex<stdout>>` for usage/tool_request/finished).
4. Reactive-retry vs compression decomposition (overflow currently re-compresses
   THEN retries — coupling loop-state mutation to transport retry).
5. Transport DB-purity (hoist ChatGPT `reasoning_effort` read out).
6. #166 discipline across the new crate boundary.
