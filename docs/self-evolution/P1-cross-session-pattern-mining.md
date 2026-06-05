# P1 — Cross-session pattern mining (detailed design)

> Phase 1 of self-evolution (see `README.md`). This is an implementation-ready
> spec: a developer or agent (incl. Codex) should be able to build it from this
> document alone. File/line references are to CodeFactory's tree at time of
> writing — verify against current code before editing.

## 1. Motivation

Reflection today is **per-session**: `commands/learning.rs::run_postmortem`
looks at one session's task outcomes and proposes 0–3 learnings. It cannot see
what only emerges **across** sessions:

- "the `bash` tool fails ~40% of the time in this project, usually on `pwsh`"
- "task decompositions that include an explicit verification step pass review
  far more often"
- "the user accepts `memory` learnings but almost always rejects `preference`
  ones" (so stop proposing the latter)

P1 adds a **cross-session miner**: it aggregates the outcome data CodeFactory
already records and turns recurring, evidence-backed patterns into higher-
quality learnings — and into signals later phases (P2 skills, P3 tuning, P4
self-mod) consume.

## 2. Scope

**In scope**
- A new aggregate analysis over a window of sessions for a given `cwd`.
- Deterministic SQL-based pattern extraction + an optional single LLM pass to
  phrase patterns as user-facing learnings.
- Emitting results into the existing learning pipeline with an **evidence /
  support count**, so they flow to chat via A1–A3.

**Out of scope (later phases)**
- Acting on patterns automatically (P2+). P1 only *surfaces* them for review.
- Semantic clustering of free-text (needs embeddings — Direction B). P1 uses
  structured fields + exact/categorical grouping.

## 3. Data sources (all already persisted)

| Source | Table | Useful fields |
|---|---|---|
| Task outcomes | `task_runs` | `status`, `attempt_count`, `error`, `verification_results` (JSON), `cwd`, `created_at` |
| Tool usage | `tool_calls` | `tool_name`, `status`, `error`, `duration_ms` |
| Human signal | `learning_events` | `kind`, `status` (accepted/rejected), `suggestion`, `decided_at` |
| Retrieval | `retrieval_events` | `query`, `result_refs_json`, `latency_ms` |
| Cost | `cost_entries` | `model`, `input_tokens`, `output_tokens`, `cost_usd` |

Schema lives in `src-tauri/src/storage/db.rs` and `migrations/`. No new source
data is required for P1.

## 4. The miner

A pass `mine_cross_session_patterns(cwd, window)` that runs the detectors below
over the last `window` sessions (default: 30 days OR last 50 sessions for the
`cwd`, whichever is smaller). Each detector yields zero or more **PatternInsight**
records; insights below a support threshold are dropped.

### Detectors (deterministic, SQL-first)

1. **Tool reliability** — group `tool_calls` by `tool_name`; compute
   error-rate and the top error substring. Emit when `calls >= 8` and
   `error_rate >= 0.25`. Insight: *"`{tool}` failed {rate}% ({n}/{total});
   most common: {error_excerpt}."*
2. **Retry-prone tasks** — `task_runs` where `attempt_count >= 2`; bucket by a
   normalized title/error category. Emit when a category recurs `>= 3` times.
3. **Verification failure shapes** — parse `verification_results` JSON; find
   the most common failing dimension/criterion across tasks. Emit when it
   recurs `>= 3`.
4. **Learning-acceptance calibration** — from `learning_events`, compute
   accept-rate per `kind`. Emit a *meta* insight when a `kind` has
   `decided >= 5` and `accept_rate <= 0.2` ("stop proposing kind=X") or
   `>= 0.8` ("prefer kind=X"). This insight tunes future `run_postmortem`
   prompts (see §7).

> Detectors are pure functions over query results so they unit-test without a
> live model or endpoint.

### Optional LLM phrasing pass

Detectors produce a terse fact + numbers. A single, capped LLM call (reuse the
`run_postmortem` request plumbing: `http_util::post_chat_completions`,
`temperature=0.3`, `max_tokens<=500`) rewrites them into one-line `suggestion`s
suitable for the UI. If the call fails, fall back to the deterministic phrasing
— the miner must never hard-depend on the network.

## 5. Data model

Reuse `learning_events` (so insights flow through A1–A3 unchanged) plus two
additive columns — additive so the existing `ensure_schema` ALTER-TABLE pattern
in `db.rs` applies with no destructive migration:

```sql
ALTER TABLE learning_events ADD COLUMN support_count INTEGER DEFAULT 0;
ALTER TABLE learning_events ADD COLUMN evidence_json TEXT DEFAULT '{}';
-- existing rows default to support_count=0 (per-session post-mortem),
-- mined insights set support_count = N and evidence_json = {detector, metrics}.
```

- `kind` gains a value `'pattern'` (alongside `'memory'`/`'preference'`) for
  mined insights. Accepting a `pattern` insight routes like `memory` (appended
  to `.codefactory/memory.md`) unless it carries a `pref_key` (then like
  `preference`). Reuse `accept_learning_event` (`learning.rs`) — only the
  routing switch needs the new `kind`.
- The A3 dedup (`norm_suggestion` in `learning.rs`) already prevents a mined
  insight from duplicating an existing learning.

## 6. Commands / API

New Tauri commands in `commands/learning.rs` (register in `lib.rs`):

- `mine_cross_session_patterns(cwd: String) -> Vec<LearningEvent>` — runs the
  detectors + optional phrasing, inserts `status='pending'` rows with
  `support_count`/`evidence_json`, emits `learning_events_updated:{cwd}` (same
  event the UI already listens to). Idempotent: dedup via `norm_suggestion`
  against existing accepted/pending, like A3.
- Trigger points: (a) manually from the Profile page; (b) opportunistically
  after every K-th completed session (counter in the session-complete path that
  already calls `run_postmortem`); (c) optional daily schedule.

## 7. Feedback into the per-session post-mortem

The "learning-acceptance calibration" insight (detector 4) is fed back into
`run_postmortem`'s prompt (§4 of `learning.rs`): include a line like *"the user
tends to reject `{kind}` learnings — only propose them when very confident."*
This closes a tight observe→reflect→adapt sub-loop on the proposer itself.

## 8. Frontend

`src/pages/Profile/ProfilePage.tsx` (Learning Log section) already renders
`learning_events`. Additive changes:
- Show a **support badge** ("支持证据: N 会话") when `support_count > 0`.
- A **"分析跨会话模式"** button calling `mine_cross_session_patterns(cwd)`.
- Render the A3 `⚠️ 与现有冲突` prefix distinctly if present.

`src/stores/learning.ts` already subscribes to `learning_events_updated:{cwd}`
— no new wiring needed beyond the optional fields.

## 9. Implementation tasks (ordered)

1. **Schema**: add `support_count` + `evidence_json` columns via the
   `ensure_schema` ALTER pattern in `db.rs`; thread them through the
   `LearningEvent` struct + the `list_learning_events` query mapping.
2. **Detectors**: pure functions `fn detect_*(rows) -> Vec<PatternInsight>`,
   each unit-tested with hand-built rows. No DB/LLM in the unit tests.
3. **Miner**: `mine_cross_session_patterns` — query the window, run detectors,
   apply support thresholds, dedup (`norm_suggestion`), optional LLM phrasing,
   insert pending rows, emit the update event.
4. **Accept routing**: extend `accept_learning_event` for `kind='pattern'`.
5. **Post-mortem feedback**: fold detector-4 calibration into the
   `run_postmortem` prompt.
6. **Frontend**: support badge + "分析跨会话模式" button + conflict styling.
7. **Trigger**: every-K-sessions counter (+ optional daily schedule).

## 10. Acceptance criteria

- Running the miner on a fixture DB with a known-flaky tool yields a
  `kind='pattern'` learning naming that tool with the correct `support_count`
  and an `evidence_json` carrying the detector + metrics.
- Detectors are deterministic and unit-tested (no live model/endpoint), mirroring
  the existing `learning.rs` storage-only test style.
- An accepted `pattern` insight appears in chat via the A1 injection path
  (it lands in memory.md / preferences exactly as a `memory`/`preference`
  learning does).
- The miner never hard-fails on a missing endpoint (LLM phrasing is best-effort,
  detectors are offline).
- No destructive migration: existing `learning_events` rows keep working with
  `support_count=0`.

## 11. Risks & guardrails

- **Noise / false patterns** → support thresholds + the human review gate
  (insights land `pending`, never auto-accepted). Cite evidence counts.
- **Cost** → at most one capped LLM call per mining run; detectors are SQL.
- **Privacy** → mining stays per-`cwd` and local; anonymous sessions are
  excluded (they already bypass learning).
- **Scope creep into P2** → P1 only *surfaces* patterns. Auto-creating skills
  from them is explicitly P2.

## 12. Test plan

- Unit: each detector over hand-built row sets (happy + threshold edges +
  empty). `norm_suggestion` dedup already covered by A3 tests.
- Integration (storage-only, in-memory SQLite like existing `learning.rs`
  tests): seed `tool_calls`/`task_runs`, run the miner with LLM phrasing
  stubbed off, assert the inserted `learning_events` rows + columns.
- Manual: drive the Profile "分析跨会话模式" button in the running app and
  confirm a support-badged insight appears and, once accepted, shows up in a
  new chat's context.
