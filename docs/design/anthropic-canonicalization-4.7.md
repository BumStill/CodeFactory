# Slice 4.7 — Anthropic canonicalization

Design from workflow `wf_b4d1af51` (4 mapper agents + synthesis, max-effort). The
**hardest** slice and the **one deliberately non-transparent** change of the
unified-agent-loop refactor. Converts `AgentLoop::run_anthropic` (~1200 lines,
`serde_json::Value` history) to canonical `ChatMessage`, pushing the
`Value`↔Anthropic-wire conversion to the transport EDGE, so `run_anthropic`
drives the SAME `run_agent_loop` as `run_openai`.

## Strategy (pure edge-conversion — NO ChatMessage changes)
- `run_anthropic` stops building `Vec<Value>` and uses the SAME
  `build_openai_messages(history, system_prompt)` as the OpenAI adapter (system →
  `messages[0]`, images → `image_url` data-URL parts, assistant text+tool_calls
  flat, each tool row a `role:"tool"` ChatMessage).
- A new PURE fn `chat_messages_to_anthropic(&[ChatMessage]) -> (String system,
  Vec<Value> messages)` — the inverse of `build_anthropic_messages` + the
  live-append shaping — runs INSIDE the Anthropic `ModelTransport::complete`,
  producing the exact wire array. Extracts leading `role=="system"` → top-level
  `system`; **merges each maximal run of consecutive `role:"tool"` ChatMessages
  into ONE `{role:"user", content:[tool_result…]}`** (the deliberate
  non-transparent merge); reconstructs assistant blocks (text-if-nonempty then
  `tool_use` with `input = from_str(args).unwrap_or({})`, empty-both →
  `[{text:""}]`); converts `image_url` data-URLs back to `{type:image, source:
  {base64, media_type, data}}`.
- `stream_anthropic` (anthropic_client.rs) stays BYTE-IDENTICAL; only its caller
  moves from `AgentLoop::call_anthropic_transport` into `DesktopModelTransport`.
- `run_anthropic` collapses to an ~83-line adapter (like `run_openai`) with
  `context_compression=false`, `overload_backoff=true`, and a
  `DesktopContextPolicy{expand_context_window:false}` (reports `default_limit`).

## Parameterization (must NOT regress the OpenAI path)
- `RunConfig.context_compression: bool` (openai=true, anthropic=false) — gates
  the estimate→compress→repair→ContextCompressed block + the overflow
  emergency-compress arm.
- `RunConfig.overload_backoff: bool` (openai=false, anthropic=true) — a NEW
  reactive arm: `is_provider_overloaded` → persist one `turn_notice` → 20s/40s
  cancel-aware retries → re-call `complete` (no `StreamEvent`).
- Keep the vision-rejection arm + proactive `supports_vision` strip active for
  both (`strip_image_parts` is the ChatMessage twin of `strip_image_values`).
- `DesktopContextPolicy.expand_context_window: bool` (openai=true, anthropic=
  false) — `false` → `context_window` returns `default_limit`, byte-identical to
  today's Anthropic context bar.

## Transport
- `DesktopModelTransport::complete` gains `match self.api_style { Anthropic =>
  call_anthropic_model, _ => call_openai_transport }` (today's `_` would
  mis-route Anthropic to the OpenAI model — MUST branch).
- The required→auto tool-choice fallback moves INTO `complete()`.
- Exact wire body preserved: `max_tokens: 8096` (NOT 8192), `system`, merged
  `messages`, `tools` (openai_tools_to_anthropic), `tool_choice:{type:any}` iff
  `require_tool && tools`. Headers `x-api-key` + `anthropic-version: 2023-06-01`.
  Silent `send_with_retry` (no TransportRetry events, invariant #9).
- `AnthropicResponse → ModelResponse`: usage gated on `(input>0||output>0)`;
  `reasoning=None`; error `AppError → TransportError::Fatal(e.to_string())`
  verbatim so the loop's overflow/vision/overload greps still fire.

## The 8 sub-steps (only 4 + 7 are non-transparent; both golden-pinned)
1. **(low/T)** Relocate `is_provider_overloaded` → agent-loop; re-export in bin.
2. **(med/T)** Add `context_compression`+`overload_backoff` to RunConfig; gate the
   compress block + add the backoff arm. openai byte-identical (both flags select
   today's behaviour).
3. **(low/T)** Add `expand_context_window` to DesktopContextPolicy.
4. **(high/N)** Write + golden-PIN `chat_messages_to_anthropic` (`#[allow(dead_code)]`,
   not wired). The isolated risky representation switch, fully unit-pinned.
5. **(med/T)** Pin `stream_anthropic` with SSE parser tests before it moves.
6. **(med/T)** Wire the Anthropic arm into `complete()` (dormant; run_anthropic
   still on the old path).
7. **(high/N)** Flip `run_anthropic` to a thin adapter through `run_agent_loop`;
   DELETE the ~1200-line body + `build_anthropic_messages` +
   `call_anthropic_transport`. The single step where representation + wire shape
   change; assert the enumerated non-transparent shifts.
8. **(low/T)** Delete dead Value code (`strip_image_values`,
   `extract_anthropic_blocks`, `AnthropicResponse.cancelled`); repoint stale tests.

## Non-transparent behaviours (release notes)
- **Replayed multi-tool turns change wire shape**: rebuilt histories with ≥2 tool
  results/assistant-turn go from N one-block user messages to ONE N-block user
  message. Functionally equivalent (Anthropic merges consecutive same-role
  messages server-side) — and it's the shape the LIVE loop already sends.
- Assistant empty-text handling unifies (only affects replay of an empty-both
  assistant, which the live loop never persists).
- Anthropic assistant message DB rows gain `input_tokens`/`output_tokens` columns
  (were NULL). Harmless/improvement.
- Overload backoff relocated from the method into the shared loop arm (schedule +
  cap + turn_notice + no-event all preserved).
- Per-response `cancelled` bool dropped (cancellation flows via the shared
  `Arc`, equivalent). tool_calls cleared on cancel for OpenAI parity (unobservable).
- Compression stays OFF for Anthropic **by design** (preserved, not changed).

## Open risks
- Strict user/assistant alternation: the tool-run merge must be surgically limited
  to contiguous `role:tool` runs and NEVER absorb a following `role:user`.
- data-URL parsing: assumes `data:<mime>;base64,<data>`; a raw-http image_url needs
  a defensive `{type:image,source:{type:url,url}}` fallback.
- `input==0 && output>0` early-cancel corner: the shared loop may emit one extra
  `ContextUsage{used:0}` (unreachable on completed turns). Pin + document.
- **Extended-thinking / signed thinking blocks**: `reasoning_content:Option<String>`
  can't carry a thinking signature. 4.7 neither requests nor parses thinking
  (reasoning stays None) → no regression, but a hard type-gap if extended thinking
  is ever enabled. Out of scope; acknowledged.
- **Real-runtime validation**: a live Claude round-trip (multi-tool turn, image
  turn, induced overload) is the ideal final check — CANNOT be run in this
  environment. Mitigated: the merged tool_result shape is exactly what the live
  loop already sends today, and every wire shape is golden-pinned. Recommend the
  user exercise a real Anthropic round-trip post-ship.
