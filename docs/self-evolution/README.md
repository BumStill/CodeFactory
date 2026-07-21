# Self-Evolution — overview & roadmap

> **2026-07-21 status:** 观察层已闭合（v1.48.0 起）——真实聊天/agent 循环对每个
> 声明的工具调用写入规范化 `tool_calls`（`agent/mod.rs` → `trajectory.rs`），reflect
> 层从 `tool_calls` 消费（`commands/learning.rs`）；anonymous 会话正确排除。因此上一版
> 「真实观察层未闭合」的审计结论**已不再成立**。
>
> 会话结束后的**本地确定性跨会话挖掘**（`mine_cross_session_patterns`，无模型调用、
> 数据不出本机）现在**默认自动运行**，因此差异化的 reflect→memory 闭环**开箱即用**、
> 会产出有证据支撑的候选进人工「进化审查」。发送脱敏摘要给模型的**远程复盘**
> （`run_postmortem`）仍严格 opt-in（`remote_postmortem_enabled`，默认关）。
>
> 仍诚实存在的缺口（非本文档吹的 shipped）：接受后的记忆缺**曝光追踪/退化**
> （只增不减，见 `docs/BACKLOG.md`）；P4 的自主 implement→verify→PR 明确**未实现**，
> 只有只读提案。

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
| **P1 — cross-session pattern mining** ✅ | memory quality | 规范化轨迹已闭合（v1.48.0），确定性挖掘会话结束后**默认自动运行**（`mine_cross_session_patterns`，无模型调用），候选进人工进化审查。剩余增量：记忆曝光追踪/退化（`docs/BACKLOG.md`）。See `P1-cross-session-pattern-mining.md`. | low |
| **P2 — skill auto-evolution** ✅ | agent capability | **Shipped** (`propose_skills_from_patterns` — writes *disabled* proposal skills the user previews + enables; never auto-enables). A recurring task pattern → auto-propose/refine a skill. Reuses the existing skill system. See `P2-skill-auto-evolution.md`. | medium |
| **P3 — self-tuning** ⚠️ | routing/policies | 自校准逻辑存在；其依赖的规范化轨迹已闭合（v1.48.0），工具可靠性/门控统计已能读到有数据的 `tool_calls`。仍未完整 shipped：激活效果的评估与回归证据。See `P3-self-tuning.md`. | medium |
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
