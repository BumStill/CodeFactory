# Slice 4.6b — relocate the loop body into `agent-loop::run_agent_loop`

Design from workflow `wf_ddc87d7d` (4 mapper agents + 1 synthesis architect, max-effort).
Physically lifts the desktop `AgentLoop::run_openai` body (~596 lines,
`src-tauri/src/agent/mod.rs`) into a tauri-free
`agent-loop::run::run_agent_loop(inputs, config, svc) -> Result<RunOutcome, LoopError>`,
reducing `run_openai` to a thin bin adapter. This is what finally makes the real
loop callable head-less (the sidecar wires onto it in a later slice).
`run_anthropic` stays on its own path (shares every helper — so **nothing may be
deleted**, only moved-and-re-`use`d).

## What the 4.6 seams already gave us
The loop already drives transport / tools / persistence / events / permissions /
hooks / context through trait objects. The mapping found the residual coupling is
**smaller than feared**:
- **Persistence family is already 100% trait-routed** (4.4b): `persist_message`,
  `persist_gate_message(_once)`, `mark_rejected_candidate`,
  `record_tool_call_outcome`, `record_tool_call_started`,
  `persist_cancelled_tool_batch`, `record_usage` all have trait twins. The bin
  inherent wrappers only add a `Usage→(i64,i64)` token split + `to_app_error`
  mapping. **Zero new Persistence methods.**
- **Exactly ONE new trait: `FactChecker`** — `fact_check_reply` runs mid-loop and
  probes the machine (delivery/PATH), tauri-free but side-effectful, so it hides
  behind a 1-method sync trait. Desktop impl holds `mode`; `NoOpFactChecker`
  returns `None`.
- **`self.app` has exactly TWO body build sites** — hooks (`match &self.app …`)
  and `DesktopToolBackend` — both move verbatim into the bin adapter and enter
  `svc` as `Arc<dyn …>`. Neither enters agent-loop (#166 preserved).
- **`self.execution_context` / `self.mcp_manager`** are eliminated by pre-deriving
  their scalars in the adapter (`task_id`, `knowledge_library_ids`,
  `audit_session_id`, `surface`) and folding `mcp_manager` into the tool backend.

## Target shape
```rust
// agent-loop/src/run.rs
pub async fn run_agent_loop(inputs: LoopInputs, config: RunConfig, svc: LoopServices)
    -> Result<RunOutcome, LoopError>;

struct LoopInputs {           // per-turn data the adapter pre-builds
    messages, system_prompt, tool_defs, completion_instruction,
    fact_check_instruction, audit_session_id, knowledge_library_ids, cancel,
}
struct LoopServices {         // 8 existing seams + FactChecker
    transport, tools, persistence, events, budget, permission, hooks,
    context_policy, fact_checker,
}
// RunConfig gains: session_id, endpoint_name, model_id, base_url, usage_run_id,
//   surface, task_id, anonymous, is_chatgpt, wall_budget_applies, cwd
//   (finalization/recovery_limit/max_iterations/gate_benchmark/progress_window exist)
```
`is_chatgpt`/`surface` are **pre-derived bools/&str** so `ApiStyle`/`UsageSurface`
(bin types) never enter agent-loop. `RunOutcome` is assembled at each terminal but
**discarded by the desktop adapter** (it already emitted `Done`) — must not alter
any emitted event or persisted row.

## The 18 sub-steps (physical move LAST; full suite after each)
1. Relocate `cancelled_tool_suffix` → agent-loop; re-`use` in bin.
2. Relocate `strip_image_parts` / `is_vision_rejection` / placeholder (+`strip_image_values`).
3. Relocate `repair_openai_tool_protocol` (+ nested helpers).
4. Relocate the pure context trio `compress_if_needed`/`estimate_prompt_tokens`/`is_context_overflow` (+`CompressionResult`, constants); leave Settings-based `resolve_context_window`/`model_supports_vision` in bin.
5. Relocate `record_completion_outcome`, changing its `&tools::ToolOutput` param to `(&str, bool)`; update BOTH loops.
6. Extract `UsageIdentity` + `record_usage_event_for_round` + `usage_request_id` crate free fns; bin inherent method delegates (keeps run_anthropic + the `record_usage_event` source-text substring).
7. `let events = self.events.clone();` → rewrite 15 emit sites (leave `emit_cancelled_done` for 16).
8. `let persistence = self.persistence();` → rewrite persist_*/mark_rejected/record_tool_call_* sites (keep `to_app_error` until the move; RAW-vs-REDACTED must not cross-wire).
9. `let transport: Arc<dyn ModelTransport> = Arc::new(self.model_transport());` → 3 sites.
10. `let context_policy: Arc<dyn ContextPolicy> …` → 3 sites (live re-read preserved).
11. `let permission: Arc<dyn PermissionGateway> …` → 1 site.
12. Hoist `DesktopToolBackend` to one shared `Arc<dyn ToolBackend>` (keep `#[cfg(not(test))] app`).
13. Drop `tools::ToolOutput` down-map → use crate `ToolInvocationResult` directly (status strings byte-identical).
14. Introduce `FactChecker` trait + `DesktopFactChecker`/`NoOpFactChecker`; rewrite the mid-loop `fact_check_reply` site.
15. Bind mode→policy scalars (`finalization`/`recovery_limit`/`wall_budget`); rewrite the 6 policy call sites.
16. Route cancel (`is_cancelled(cancel.as_ref())`, SeqCst, same `Arc`) + inline `emit_cancelled_done`; update the openai cancel source-text marker.
17. Bind remaining scalars (`cwd`/`audit_session_id`/`task_id`/`knowledge_library_ids`/`usage_run_id`/`usage_identity`); switch usage call to the crate free fn (keep `record_usage_event` substring). After this the body references only locals + crate/agent-core items + the top-of-fn message build.
18. **Physical move**: create `run_agent_loop`, cut the body verbatim, swap locals→`inputs/config/svc`, `?`→`LoopError`, assemble `RunOutcome`; reduce `run_openai` to build-inputs/config/svc + `run_agent_loop(…).await.map_err(AppError::from)?`. Re-point the openai case of both usage source-text tests to `run.rs`.

## Byte-identical invariants (must not regress)
- **Source-text acceptance markers** (`usage_acceptance_tests.rs`): keep the
  `for iteration in 0..max_iterations`, `record_usage_event`, `if tool_calls.is_empty()`,
  `} = match call_result` literals; the `if self.is_cancelled()` marker changes in
  step 16; at step 18 re-point openai to `run.rs`, anthropic stays on `mod.rs`.
- **RAW vs REDACTED**: `persist_message` redacts; `persist_gate_message(_once)` write
  verbatim. `state=="turn_notice"`→role system (replay-excluded), else role user.
- **Usage ordering**: `record_usage_event_for_round` awaited BEFORE the post-response
  cancel check AND before `if tool_calls.is_empty()`; idempotent on
  `request_id = usage_run_id:iteration`; `usage_recorded` only on `record_usage Ok(true)`;
  anonymous + (0,0)-token skip before assembly.
- **Cancel**: every load `SeqCst`, the SAME `Arc<AtomicBool>` shared with transport +
  permission; `cancelled_tool_suffix` index-0 drives the cancelled-vs-skipped split;
  terminal `Done{0,0}` emitted exactly once.
- **#166**: `HookRunner` + `DesktopToolBackend` (AppHandle owners) constructed ONLY in
  the bin adapter under `#[cfg(not(test))]`; after step 18
  `cargo tree -p codefactory-agent-loop | grep -i tauri` is EMPTY.
- **run_anthropic shares everything** — no deletions; re-`use` moved names into bin.
- **knowledge scope** `None` vs `Some([])` preserved; `audit_session_id` parent-first.
- Both reactive-retry branches (overflow, vision) covered by the relocations/locals.
