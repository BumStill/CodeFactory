# Backlog

Things noted during work that aren't in the current PR. Sorted newest-first.
When picking one up, move it to its own PR scope and remove from here.

The entries below came out of the 2026-07-15 openJiuwen deep-dive
(openJiuwen-ai/agent-core v0.1.16, jiuwenswarm v0.2.3 — Apache-2.0, so
referenced mechanisms can be studied directly). Ordered by priority.

## Exposure tracking + re-scoring for accepted memory and skills (P1)

Record when `memory.md` entries and enabled skills are actually injected into
context, then periodically re-score them and propose pruning the low-value
ones through the existing review workbench.

Why: our memory only grows — nothing tracks whether an accepted learning ever
helped, so `memory.md` will rot into noise. openJiuwen's ExperienceTracker
records exposure (`record_presented_experiences`), re-scores with an LLM every
N sessions, and retires low scorers. Note the adjacent signal-extraction work
(deterministic failure/correction classifiers) is already owned by
CF-EVO-20260714 Phase 1 — `agent_evolving/signal/from_conv.py` is a good
reference implementation for that extractor; this entry covers only the
net-new decay/retirement loop for already-accepted items.

Open questions when scoping:
- What counts as "used": injected into context, or demonstrably referenced by
  the model?
- Prune via proposal through the evolution review workbench (HITL), never
  auto-delete?
- Does the same loop cover `user_preferences` rows?

## Hybrid keyword+vector retrieval for the knowledge base (P1)

Add embedding search via the sqlite-vec extension alongside the current
keyword matching in `knowledge.rs`, and blend the two scores.

Why: retrieval is keyword-only today, so recall degrades on large or
paraphrased corpora. sqlite-vec is a single C extension that drops into our
existing SQLite storage, and openJiuwen's lite memory validates exactly this
hybrid (keyword + vector, with an in-memory cosine fallback when the extension
is unavailable).

Open questions when scoping:
- Embedding source: local model vs provider API — BYO-key privacy stance
  suggests local-first or strictly opt-in remote.
- Embed at ingest or lazily on first query; behavior when no embedder is
  configured (must degrade to today's keyword path).

## Mid-term candidates from the openJiuwen benchmarking (P2 bundle)

Four smaller candidates, each validated in openJiuwen and mapped to a known
gap; split into its own scope when picked up:
- Mid-run steering: queue user messages during a run and inject them at
  iteration boundaries (today the only mid-run control is stop).
- Session rewind/fork on message-level history (their session VCS:
  append-only WAL with commit/replay/rewind).
- Scheduled autonomous runs: a heartbeat/cron mode that executes a
  project-defined task list during idle windows.
- ACP/stdio bridge to drive external CLI agents as subagents — also routes
  around the subagent-can't-use-ChatGPT/Codex limitation
  (`agent/subagent.rs` TODO).

Why: each is a proven mechanism there and a real gap here, but none blocks
the P0/P1 items above.

Open questions when scoping:
- Priority order among the four.
- Steering touches the agent-loop core — needs a risk assessment before
  scoping.

---

If you spot anything that belongs in a follow-up PR rather than the
current one, add a section above with:
- one-line summary
- 2-3 sentence why
- the open questions to resolve when scoping
