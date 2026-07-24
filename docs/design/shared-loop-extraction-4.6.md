# Slice 4.6 — extract the shared loop (`run_agent_loop`)

Design from workflow `wf_07197651-963` (5 agents, max-effort architect). The last
and largest slice of the unified-loop refactor: lift the desktop `run_openai`
body (~651 lines, `src-tauri/src/agent/mod.rs:979-1630`) into a tauri-free
`agent-loop::run_agent_loop`, driven entirely through trait seams, so it can
become one loop with the Terminal-Bench sidecar (4.8). `run_anthropic` stays on
its `serde_json::Value` path, untouched until 4.7.

## Feasibility verdict
Tractable as **ONE branch/PR** but **only as ~8 sequenced, individually-green
sub-steps with the physical file-move LAST** — a single-diff lift of 651 lines is
NOT byte-safe. Fallback: if step 8's byte-verification is shaky, stop after
step 7 (the loop stays in `mod.rs` but drives entirely through the new seams —
still tauri-free-ready, zero behaviour change; relocate later).

> **Decision (shipped): the step-7 fallback was taken deliberately.** 4.6 ships
> as sub-steps 1–7 — the loop now drives entirely through all eight capability
> seams (transport/tools/persistence/events/budget/permissions/hooks/context),
> zero behaviour change, tauri-free-**ready**. A scan of the residual `run_openai`
> body showed the "physical move" is much larger than a `~40-line adapter`: 7+
> AgentLoop inherent methods (`record_tool_call_outcome`, `persist_gate_message`,
> `mark_rejected_candidate`, `emit_cancelled_done`, `usage_request_id`,
> `build_openai_messages`, `audit_session_id`) plus `self.app`/`self.mcp_manager`/
> `self.execution_context` are still bin-coupled and must be seam-routed FIRST.
> That relocate becomes its own follow-up slice **4.6b** (see sub-step 8), not
> rushed at the tail of this one.

## Target shape
```rust
// agent-loop/src/run.rs
pub async fn run_agent_loop(
    inputs: LoopInputs,     // messages, system_prompt, completion_instruction, tool_defs, cancel
    config: RunConfig,      // finalization policy + gate flags + usage-attribution identity
    svc: LoopServices,      // the 8 trait objects
) -> Result<RunOutcome, LoopError>;

struct LoopServices {
    transport:   Arc<dyn ModelTransport>,   // ✅ 4.5b
    tools:       Arc<dyn ToolBackend>,      // ✅ 4.3
    persistence: Arc<dyn Persistence>,      // ✅ 4.4a/b
    events:      Arc<dyn EventSink>,        // ✅ 4.1
    budget:      Arc<dyn Budget>,           // ✅ 4.2
    permissions: Arc<dyn PermissionGateway>,// NEW
    hooks:       Arc<dyn LifecycleHooks>,   // NEW
    context:     Arc<dyn ContextPolicy>,    // NEW
}
enum LoopError { Transport(TransportError), Persist(PersistError), Tool(ToolError) }
```
`RunConfig` (extend `run.rs`) adds usage-attribution identity: `session_id,
endpoint_name, model_id, base_url, usage_run_id, anonymous, is_chatgpt, surface,
task_id, cwd`, plus `wall_budget_applies`. `tool_defs` stays an **input**
(desktop pre-assembles `all_definitions`+MCP+anonymous-KB-strip in `run()`), so
the desktop path is byte-identical; the sidecar passes its own `[run_shell]`.

## Three new capability traits (agent-loop; desktop impls in bin under `#[cfg(not(test))]`)
- **`ContextPolicy`** — `context_window(estimated)->(usize,usize)`, `supports_vision()->bool`,
  `round_reasoning_effort()->String`. Desktop impl re-reads `Settings`+db **each
  round** (mid-run freshness — a frozen snapshot would regress it). Headless
  returns a fixed window / false / "".
- **`LifecycleHooks`** — `pre_tool(name,args)->bool` (false=cancel), `post_tool(name,preview,ms)`.
  Desktop impl owns the AppHandle+Settings and picks disabled-vs-from_settings at
  CONSTRUCTION (the loop never sees `self.anonymous`). `NoOpHooks` headless.
- **`PermissionGateway`** — `decide(call,args,bash_cmd)->PermissionResponse`. Folds
  the Settings-coupled `decide_permission` AND the Ask/oneshot `request_permission`
  (+notify+600s await). `Deny(String)` keeps the two byte-distinct denial strings.
  Headless returns `Allow`. `autonomous_budget_denial` stays pure in the loop, BEFORE the gateway.

Plus: `EventSink::usage_recorded(session_id)` defaulted no-op (TauriEventSink emits
the two `*-usage-recorded` events, only on `Persistence::record_usage` Ok(true));
`Persistence::record_tool_call_started` (absorbs a stray raw-pool write).

## Progress (branch `claude/keystone-slice46-shared-loop`, pushed)
- **1 ✅** (ebc801f) mode-policy → `agent-loop::policy`, re-parameterized by
  `FinalizationPolicy`; thin `AgentMode` wrappers in mod.rs; #135/#136 gate tests
  byte-identical; 4 new policy tests.
- **2 ✅** (84a6a51) usage seam: `EventSink::usage_recorded` (TauriEventSink emits
  the 2 cost events) + `record_usage_event_for_round` through `Persistence::record_usage`.
- **3 ✅** (1a08902) `Persistence::record_tool_call_started` absorbs the last raw
  `self.db` write in the loops.
- **4 ✅** `ContextPolicy` (`agent-loop/services.rs`, `u32` window to match
  `context::ContextWindow`) + `DesktopContextPolicy` (`src/agent/context_policy.rs`,
  no `AppHandle` — settings lock + pool + config identity only, reads via `super::`).
  Rewires openai `context_window`/`round_reasoning_effort` and BOTH loops'
  `supports_vision`. The old `resolve_round_reasoning_effort` body moved verbatim
  (api_style gate folded in). Anthropic `context_window` stays inline — it uses
  `default_limit`, not `select_limit(estimated)`, so it waits for 4.7. 470 lib +
  16 agent-loop tests green; vision-strip + reasoning-freshness pins hold.
- **5 ✅** `LifecycleHooks` (`agent-loop/services.rs`, defaulted allow/no-op) +
  `NoOpHooks` (headless, no `AppHandle`) + `DesktopLifecycleHooks`
  (`src/agent/lifecycle_hooks.rs`, wraps `HookRunner`, built only in the
  dead-stripped loops like the old `Option<HookRunner>`). Both loops' build +
  PreTool `match` + PostTool `if let` collapse to `hooks.pre_tool`/`post_tool`.
  470 lib + 17 agent-loop tests green (added `noop_hooks_allow_all…`).
- **6 ✅** `PermissionGateway` + `PermissionOutcome` (agent-loop) +
  `AllowAllPermissions` (headless, allow-all) + `DesktopPermissionGateway`
  (`src/agent/permission_gateway.rs`, owns Arc handles only — no `AppHandle`, so
  no cfg gating). Folds `decide_permission` (stays a directly-tested free fn the
  gateway calls via `super::`) + `request_permission` (moved verbatim). Both
  loops' `let permission_policy`/`decision` + the `match decision` block collapse
  to `match self.permission_gateway().authorize(tc, &args, bash_cmd).await`; the
  Cancelled arm still returns `finish_cancelled_tool_batch` (which stays on the
  loop — it needs the batch remainder). Denial strings + warn byte-identical;
  `decide_permission` is pure so its now-lazy call is unobservable. Retargeted
  the usage-acceptance source-text end marker to `finish_cancelled_tool_batch`
  (request_permission left mod.rs). 470 lib + 18 agent-loop tests green.
- **7 ✅** transport→`complete()`: the openai loop's main call + both reactive
  retries now go through `ModelTransport::complete(&messages, tools,
  &RoundOptions{require_tool, reasoning_effort})` returning `ModelResponse`
  (destructured in place); `TransportError` crosses back as
  `AppError::Other(e.to_string())` via a new `From` in `errors.rs` (message
  verbatim, so the overflow/vision `e.to_string()` greps and all Display-only
  consumers are byte-identical — no consumer reads the variant). Added the
  forward-looking `LoopError{Transport,Persist,Tool}` to agent-loop (Display =
  underlying verbatim; run_agent_loop returns it in step 8). Anthropic transport
  untouched (4.7). Retargeted the openai usage-acceptance response marker to
  `} = match call_result`. 476 lib + 19 agent-loop tests green.
- **8 ⏳ → deferred to slice 4.6b** the FINAL relocate: move the residual body
  into `run_agent_loop`. Deliberately split out (see the Decision note above) —
  needs the 7+ residual inherent methods seam-routed first. 4.6 SHIPS at
  sub-step 7. Verify locally with
  `cargo test --lib -- --skip gh_cli_remote_reads_real_ci_status` (that one smoke
  needs HEAD pushed). One PR + one release when the branch is complete (or the
  step-7 fallback).

## The 8 sub-steps (one branch; verify each with the full suite; physical move last)
1. **Mode-policy move.** Move the 9 pure fns (`completion_finalization`,
   `active_tool_definitions`, `openai_tool_controls`, recovery-limit/attempts,
   `completion_ready_applies`, `autonomous_budget_denial`,
   `iteration_ceiling_terminal_event`, + `CompletionFinalization`) into agent-loop,
   re-parameterized by `FinalizationPolicy`/`recovery_limit`/`wall_budget_applies`
   instead of `AgentMode`; add the desktop `AgentMode->RunConfig` map; repoint BOTH
   loops; lift the specific mode-policy tests. Pure, zero behaviour change —
   de-risks the largest test surface first.
2. **Usage seam.** `EventSink::usage_recorded` + crate `build_usage_row`; route
   `record_usage_event_for_round` through `Persistence::record_usage`, gate the two
   emits on `Ok(true)`. Keep the call before the cancel check (usage-ordering tests).
3. **`record_tool_call_started`** — replace the stray `self.db` write.
4. ✅ **`ContextPolicy`** + `DesktopContextPolicy` — replace `supports_vision`,
   `context_window`, `round_reasoning_effort`. Verify vision-strip + reasoning
   freshness tests. (Anthropic `context_window` deferred to 4.7 — `default_limit`.)
5. ✅ **`LifecycleHooks`** + `DesktopLifecycleHooks` + `NoOpHooks` — replace the
   `hook_runner` Option/match.
6. ✅ **`PermissionGateway`** + `DesktopPermissionGateway` — fold `decide_permission`
   + `request_permission`. The fiddliest step (denial strings, Cancelled→
   `finish_cancelled_tool_batch`, pending_permissions atomicity).
7. ✅ **Transport switch** — the 3 call sites + 2 retry arms → `ModelTransport::complete`;
   introduce `LoopError`. Keep the overflow/vision arms mutating `messages` and
   grepping `e.to_string()` (TransportError Display is verbatim). `From<TransportError>
   for AppError` bridges the error back (Other, message verbatim).
8. ⏭️ **FINAL relocate → slice 4.6b** — move the residual body into `run_agent_loop`;
   reduce `AgentLoop::run_openai` to an adapter; graduate agent-loop `tokio` from
   dev-dep to real dep. Deferred (Decision note): the residual body still couples
   to 7+ inherent methods (`record_tool_call_outcome`, `persist_gate_message`,
   `mark_rejected_candidate`, `emit_cancelled_done`, `usage_request_id`,
   `build_openai_messages`, `audit_session_id`) + `self.app` — each must be
   seam-routed before the move is byte-safe. Its own PR + validation release.

## Byte-identical invariants (must not regress)
- Transport error greps stay correct because `TransportError::Display` is verbatim.
- Overflow retry: `emergency_limit = (context_limit/5).max(1)*4`, compress, retry ONCE.
- Cancellation-skips-validation: `is_cancelled` at loop-top + post-call (AFTER usage,
  BEFORE persist/gate) → `emit_cancelled_done`, no finalization.
- Usage persisted before the cancel break + before every terminal; both emits fire on Ok(true).
- Gate #135/#136: Interactive/Execute → ReleaseWithWarning (Chinese, un-verified
  wording); Autonomous → Blocked+Error; recovery 3/3/1; ready-nudge Autonomous/Benchmark-only.
- Anonymous strips stay literal (KB-tool retain, hook disabling) — never folded into Persistence.
- #166: no AppHandle-owning struct constructed in agent-loop or the unit-test EXE.
