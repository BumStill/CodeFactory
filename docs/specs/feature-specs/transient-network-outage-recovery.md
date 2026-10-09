# Spec: don't fail a task just because the network or model service dropped for a short while (U25)

> 本文是 U25 任务规格的逐字记录（task: `task-u25-transient-network.md`），落在仓库里作为
> 长期规格。实现位于 `src-tauri/src/agent/failover.rs`、`src-tauri/src/commands/chat.rs`、
> `src-tauri/src/agent/objective.rs`、`src-tauri/src/agent/failure_summary.rs`。

## Why

On 2026-10-08, between 11:35:48 and 11:38:56, requests to DeepSeek returned
`HTTP error: error sending request for url (https://api.deepseek.com/chat/completions)` for
about 3 minutes. The network recovered on its own after that. But in the meantime:

- U1 session (78e386df): the first error arrived around 11:35:48, and by 11:37:22 it had been
  ruled "这件事没做成" (`technical_recovery_exhausted`).
- U23 session (f0f973eb): ruled failed at 11:38:18.

Both had to be restarted by hand. User principle: "If one approach doesn't work, switch to
another. Only declare failure when every approach has failed." Losing the network for one minute
is not "every approach has failed".

## Root cause (verified in code)

- `src-tauri/src/agent/failover.rs`, `classify_provider_failure` (around line 118):
  transport-layer messages like `error sending request` and `timed out` are classified as
  `ProviderFailureClass::EndpointUnavailable`.
- `src-tauri/src/commands/chat.rs`, `chat_failure_code_for_error`: `EndpointUnavailable` maps to
  `PROVIDER_ENDPOINT_UNAVAILABLE`.
- `src-tauri/src/agent/objective.rs`, around line 199: `PROVIDER_ENDPOINT_UNAVAILABLE` uses
  `DEAD_ENDPOINT_RETRY_MS = 10_000`, a flat 10 seconds, combined with
  `MAX_SIGNATURE_RECOVERY_ATTEMPTS = 5`. That reaches the ceiling and settles as failed in about
  a minute.
- The reasoning in the comment there is "an endpoint that keeps answering unavailable won't fix
  itself; only switching models helps". That holds for "the service explicitly said unavailable /
  the model doesn't exist", but **not** for brief network jitter (proxy, Wi‑Fi, DNS, TLS,
  connection reset).
- `parked_incident_message` still tells the user to "switch to another available model". That is a
  parked-state message from before U3.

## Requirements Traceability

| Req ID | Requirement | Minimum evidence |
| --- | --- | --- |
| Req-1 | Separate two kinds of failure: **unreachable / transient transport** (connect / DNS / TLS / timeout / connection reset / `error sending request` / gateway 502–504) vs **the endpoint explicitly refuses** (HTTP 404 model not found, an explicit "model unavailable / deprecated" message, a persistent 503 with an explicit body). Give them separate failure codes. Auth (401/403) and insufficient balance (402) keep their own paths. | Classification table test: `error sending request`, `connection refused`, `dns error`, `operation timed out`, `connection reset`, and 502/503/504 without an explicit body → transient transport; `model not found` / an explicit unavailability statement → explicit refusal. |
| Req-2 | Transient transport gets a reasonable window: keep retrying with a growing backoff for at least 10–15 minutes (constants are the implementer's call; explain them in the PR). Before each retry, run a cheap connectivity probe (e.g. a HEAD/connect to the same base URL); if it still can't connect, just wait — don't run a full model turn and don't burn the recovery budget. Don't count one network outage as N independent failures. | Synthetic: transient transport failures for 3 minutes, then recovery. The objective must not end up failed; after the network returns it resumes and completes, and the recovery budget isn't exhausted by that one outage. |
| Req-3 | Use other routes when there are some. If the session's model policy allows it (`prefer` / `auto`, with a usable candidate route), switch to another available route first. Under `fixed`, never switch models silently; the user chose it. | Synthetic: under `fixed`, no model switch happens. Under `auto` with a usable candidate, it switches routes first. |
| Req-4 | Fail honestly once the window expires: settle into U3's failed terminal state (no new intermediate state the user can't understand). The text must say, in plain language, how long the network/service was unreachable and how many times it was tried; that the progress has been saved; and how to continue once the network is back. Clean up the outdated wording in `parked_incident_message`. Do not change `failure_summary.rs`'s handling of `completion_evidence_incomplete` (U1b just changed it). | Synthetic: still unreachable past the window → settles into the failed terminal state, and the text includes how long it was unreachable and how many attempts were made, with no internal vocabulary. |
| Req-5 | Explicit endpoint refusal (the second kind in Req-1) can keep the existing fast-convergence behaviour. | Existing endpoint-refusal tests stay green: `DEAD_ENDPOINT_RETRY_MS` fast convergence is unchanged for refusal-class failures. |

## Applicable Harnesses

- Spec Harness（必须）：本文件 + PR 中的 Req ↔ test 映射。
- Compatibility Harness：`ProviderFailureClass` 是新枚举分支，做穷尽 match 的地方必须一并更新；既有 `EndpointUnavailable` 断言不得回归。
- Observation Harness：失败文案走 `assert_no_internal_vocabulary` 守卫，落库文案必须同时有可读的时长与次数。
- 不适用 Viewport Harness：本变更不涉及布局、滚动、焦点、输入、流式渲染等前端行为。

## Test matrix

| Case | Kind | Expected |
| --- | --- | --- |
| Classification table (`error sending request`, `connection refused`, `dns error`, `operation timed out`, `connection reset`, body-less 502/503/504) | unit | `ProviderFailureClass::TransportUnreachable` → `PROVIDER_TRANSPORT_UNREACHABLE` |
| Classification table (`model not found`, explicit unavailability statement, 503 with explicit body, `circuit_open`) | unit | `ProviderFailureClass::EndpointUnavailable` → `PROVIDER_ENDPOINT_UNAVAILABLE` |
| Synthetic 3-minute transport outage then recovery | objective | Not failed; resumes and completes; exactly one real retry queued; recovery budget not exhausted |
| Synthetic outage beyond the window | objective | Failed terminal state + `provider_transport_unreachable`; message carries duration/attempts/saved-progress/continue hint; passes the internal-vocabulary guard |
| Probe still unreachable | objective | `waiting_system`, zero queued/waiting remediation, `next_observation_at` in the future, no model turn |
| Model policy `fixed` | objective | No model switch |
| Model policy `auto` with usable candidate | objective | Switches route first |
| Full workspace run | `CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-u25" cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast` | 0 failed |

## Constraints

- Base your work on the latest main. Work only on the workspace's current managed branch. Don't
  create a new branch.
- This task stays within: `failover.rs` (classification); `chat_failure_code_for_error` in
  `commands/chat.rs`; the retry policy and `parked_incident_message` in `agent/objective.rs`;
  plain-language text for the new failure codes in `failure_summary.rs` (add mappings only).
  Delivery-related files stay as they are.
- Rust builds use a dedicated build directory (`.codefactory-cache/cargo-target-u25`); don't use
  `node scripts/cargo-shared.mjs`.
- Fixtures must be synthetic data only. Don't make real network requests; use a local fake server
  or inject errors. Don't modify `docs/testing/scenario-registry.json`, the validators, or the
  workflows under `.github/`.

## Implementation Notes

- `MAX_TRANSPORT_RECOVERY_ATTEMPTS = 8` 高于 `MAX_SIGNATURE_RECOVERY_ATTEMPTS = 5`，这样一次
  网络中断不会吃完整恢复预算；退避为 15s → 45s → 135s → 4min，与 15 分钟窗口匹配。
- 传输类探针只做一次 TCP connect（`TRANSPORT_PROBE_TIMEOUT = 5s`），失败时仅推后
  `next_observation_at`（`DecisionEnvelope::transport_probe_wait`），不插入 remediation，因此
  不触发模型回合、不扣减恢复计数。
- 这些是取舍说明，不改变任何 requirement 或 acceptance criterion。
