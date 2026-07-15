# Self-Evolution — overview & roadmap

> **2026-07-14 implementation truth audit:** P0–P4 的部分逻辑和 UI 已存在，但
> v1.43.0 的真实 Agent 路径没有写入规范化 `tool_calls`，普通聊天 post-mortem
> 也只读取 `task_runs`。因此 P1/P3/P4 暂时只能视为“实现框架已存在、真实观察层
> 未闭合”，不能以 shipped 文案作为产品完成证据。修复顺序与验收见
> `docs/specs/feature-specs/evolution-agent-closed-loop.md`。
> `codex/evolution-agent-loop` 已完成本地 Trace Truth 首个切片，以及一级「进化审查」
> 工作台、人工采纳/拒绝、持久 job/event 日志和重启中断终态的真实 Dev App 证据。
> 但在本轮 PR+CI、合并、刻意发版与发布包主路径验证前仍是 `not live`；通用
> Evals/activation、versioned review、Quick 稳定 scope 和其余底座证据仍未完成，
> 不能提前恢复完整 shipped 声明。

> CodeFactory's tagline is **软件工厂 · 本地助手 · 自进化**. "Self-evolution" is
> the system by which the product gets better from its own use — the factory
> improving the factory. This document is the durable north star: it defines
> the loop, the phased roadmap, and the safety model so the work can be picked
> up by anyone (human or agent, including Codex) without re-deriving the design.

## The loop

Self-evolution is one closed feedback loop, applied at widening scopes:

```
   Observe ─────────► Reflect ─────────► Adapt ─────────► better outcomes
   (what happened)    (what to learn)    (change behavior)        │
        ▲                                                         │
        └─────────────────────────────────────────────────────────┘
```

- **Observe** — capture outcomes: task success/failure, retries, verification
  results, tool errors, cost, user accept/reject of learnings, retrieval hits.
- **Reflect** — extract lessons from those outcomes.
- **Adapt** — turn lessons into changes to behavior, at increasing scope:
  memory → skills → tuning → (ultimately) the product's own code.

## What already exists (the substrate)

| Loop stage | Already in CodeFactory |
|---|---|
| Observe | `task_runs` (status, attempt_count, error, `verification_results`), `learning_events`, `tool_calls` (name/status/error/duration), `cost_entries`, `retrieval_events`, `checkpoints` |
| Reflect | per-session post-mortem (`commands/learning.rs::run_postmortem`) |
| Adapt (memory) | preferences + accepted learnings injected into every chat (A1), under a unified context budget (A2), deduped + conflict-flagged (A3) |

**The memory system (A1–A3) is the first closed turn of this loop** — reflect →
memory → behavior. Everything below widens the "Adapt" scope.

## Roadmap (scope of adaptation grows; risk grows with it)

| Phase | Adapts | Gist | Risk |
|---|---|---|---|
| **P0 — substrate** ✅ | memory/preferences | Memory system A1–A3 (done) | low |
| **P1 — cross-session pattern mining** ⚠️ | memory quality | Command/UI 已存在；在 Phase 0 真实写入规范化轨迹并完成 live verification 前，不视为 shipped。See `P1-cross-session-pattern-mining.md`. | low |
| **P2 — skill auto-evolution** ✅ | agent capability | **Shipped** (`propose_skills_from_patterns` — writes *disabled* proposal skills the user previews + enables; never auto-enables). A recurring task pattern → auto-propose/refine a skill. Reuses the existing skill system. See `P2-skill-auto-evolution.md`. | medium |
| **P3 — self-tuning** ⚠️ | routing/policies | 自校准逻辑存在；工具可靠性/门控依赖 Phase 0 真实轨迹，验证前不视为完整 shipped。See `P3-self-tuning.md`. | medium |
| **P4 — self-modification** | **its own code** | 只读提案 UI/命令存在；摩擦数据同样依赖 Phase 0。自主 implement→verify→PR 仍明确未实现。See `P4-self-modification.md`. | high |

## Safety model (non-negotiable for every phase)

Self-evolution without guardrails is how you get drift and damage. Every phase
inherits these:

1. **Human-in-the-loop.** Behavior/code changes require human approval (P4
   always; memory/preference changes use the existing *preview-then-enable*).
   The system **proposes**, the human (or an explicit policy) **disposes**.
2. **Verification gate.** Every self-applied change passes the `verify` skill
   (run it, observe it) and CI before it lands. Nothing ships red. (Example:
   PR #72 was caught by CI before merge — that gate is the point.)
3. **Reversibility.** Every step is undoable — `checkpoints` already exist;
   memory/skills changes are revertable; code changes are PRs, not pushes.
4. **Bounded autonomy + audit.** Rate/scope caps on what the system may change
   per window, and a full audit trail (who/what/why) for every adaptation.
5. **Evidence over vibes.** An adaptation must cite the outcomes that justify it
   (e.g. "支持证据: 7 sessions"), never a single anecdote.

## Where to start

**P1.** It builds directly on the memory system (A1–A3), is low-risk, and its
output (recurring, evidence-backed patterns) is exactly the input that P2
(skill suggestions) and P4 (self-modification) need. P4 is the most ambitious
and must come last, behind the heaviest guardrails.
