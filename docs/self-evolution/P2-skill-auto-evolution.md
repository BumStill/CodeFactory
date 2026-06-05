# P2 — Skill auto-evolution (detailed design)

> Phase 2 of self-evolution (see `README.md`). Implementation-ready spec — a
> developer or agent (incl. Codex) should build it from this doc alone.
> File/line references are to the tree at writing time; verify before editing.

## 1. Motivation

P1 mines *outcome* patterns (a tool fails, an error recurs). P2 mines *workflow*
patterns — "you keep doing the same kind of task" — and turns them into a
**proposed skill**: a name, description, `system_prompt.md`, and slash commands
the agent drafts **for itself**, surfaced for the user to preview and enable.
This is the factory writing its own tools.

Concretely: if a project repeatedly runs tasks like "write a release PR
description", "add a Tauri command + register it", or "summarize a column in an
xlsx", P2 notices the recurring shape and proposes a reusable skill that
captures it — instead of the user re-explaining it every time.

## 2. Scope

**In**: detect recurring task/intent shapes across sessions; draft a skill
proposal (one LLM pass); store it **disabled** for preview-then-enable; surface
it in the Skills UI with its rationale + evidence.

**Out (later phases)**: auto-*enabling* a skill (never — preview-then-enable is
the user's standing rule); editing the project's own code (P4); semantic intent
clustering via embeddings (Direction B — P2 uses lightweight normalization).

## 3. Inputs (already persisted)

| Signal | Source | Use |
|---|---|---|
| Task shapes | `task_runs` (title, description, status, cwd) | cluster recurring intents |
| Mined patterns | `learning_events` where `kind='pattern'` (P1) | seed/strengthen proposals |
| Existing skills | user + builtin skills (`commands/skills.rs`) | don't propose what already exists |
| Spec history | `docs/specs/`, specs tables (if present) | recurring capability requests |

## 4. The proposer

`propose_skills_from_patterns(cwd, app, state) -> Vec<SkillProposal>`:

1. **Cluster recurring task intents.** Pull recent `task_runs` for the cwd;
   normalize each title (lowercase, strip ids/paths, keep head keywords — reuse
   the spirit of A3's `norm_suggestion`). Group; a cluster with `>= 4` tasks is
   a candidate.
2. **Skip what's covered.** Drop clusters already served by an enabled skill
   (compare against `list_skills` names/tags).
3. **Draft via one capped LLM pass.** For each surviving cluster, ask the model
   (reuse `run_postmortem`'s request plumbing: `http_util::post_chat_completions`,
   `temperature<=0.3`, `max_tokens<=600`) to draft a skill: `name`,
   `description`, `system_prompt` (the rule/persona), and 1–3 `slash_commands`
   ({name, description, template}). Best-effort: a failed/empty draft is
   skipped, not fatal. Validate the JSON shape before accepting.
4. **Persist as DISABLED proposals** (see §5), with a `rationale` (the cluster +
   evidence count) so the user understands *why*.
5. Emit `skill_proposals_updated:{cwd}` for the UI.

## 5. Data model

Reuse the existing skill storage. A proposal is a normal user skill written to
the user skills dir with two manifest additions (additive, no migration):

```jsonc
// manifest.json
{
  "id": "...", "name": "...", "description": "...", "version": "0.1.0",
  "author": "CodeFactory (proposed)",
  "tags": ["proposed"],
  "enabled": false,                 // ALWAYS — preview-then-enable
  "proposed": {                      // new, optional block
    "rationale": "你在本项目里做过 6 次「写发布 PR 描述」类任务。",
    "support_count": 6,
    "evidence_json": "{...}"
  }
}
```

- Lives under the user skills dir exactly like a hand-imported skill, so the
  existing scan/list/enable/delete paths in `commands/skills.rs` work unchanged.
- `enabled:false` guarantees it does nothing until the user opts in — the same
  preview-then-enable contract used for marketplace/imported skills.
- A skill the user **deletes** (rejects) shouldn't be re-proposed every run:
  keep a small `rejected_proposals` record (a JSON file under the user skills
  dir, keyed by normalized cluster) and skip those.

## 6. Commands / API

- `propose_skills_from_patterns(cwd) -> Vec<SkillManifest>` — runs §4, returns
  the new proposals (also emitted).
- Trigger: a button in the Skills page ("从我的使用习惯提议技能") and/or
  opportunistically after P1 mining. No schedule needed for v1.
- Enable/delete reuse the existing `enable_skill` / `delete_*` commands.

## 7. Frontend

`src/pages/Skills/SkillsPage.tsx`:
- A **"Proposed"** group at the top: skills with `tags` containing `proposed`,
  rendered with the `rationale` + a "支持证据: N" badge.
- Two actions per proposal: **预览**(show the drafted system_prompt +
  slash commands) and **启用**(the existing enable flow). Deleting = reject.
- Reuse the existing skill card; add the rationale line + badge.

## 8. Implementation tasks (ordered)

1. **Proposer core** (`commands/skills.rs` or a new `skill_evolution.rs`):
   the cluster + dedup-vs-existing logic as pure, unit-tested functions over
   task rows (no DB/LLM in the unit tests — mirror P1's detectors).
2. **Draft + persist**: the LLM draft pass + writing a disabled proposal skill
   dir (reuse `create_user_skill` / the write path), with the `proposed` block.
3. **Rejected-proposals guard**: don't re-propose a deleted cluster.
4. **Command + registration** in `lib.rs`.
5. **Frontend**: Proposed group + rationale/badge + preview/enable.
6. **Wire to P1** (optional): offer "propose skills" after a mining run.

## 9. Acceptance criteria

- Given a fixture with ≥4 similar task titles and no covering skill, the
  proposer yields one proposal whose manifest is `enabled:false`, tagged
  `proposed`, with a correct `support_count` + rationale.
- A cluster already served by an enabled skill yields no proposal.
- A previously-deleted (rejected) cluster is not re-proposed.
- Clustering + dedup logic is deterministic and unit-tested (no live model).
- An enabled proposal behaves as a normal skill (its system_prompt flows through
  the A2 budgeted assembly like any other skill).
- The proposer never auto-enables and never hard-fails on a missing endpoint.

## 10. Risks & guardrails

- **Bad/over-eager proposals** → high cluster threshold (≥4) + the proposal is
  *disabled*; the user previews before enabling. Evidence count shown.
- **Proposal spam** → the rejected-proposals guard; cap proposals per run (e.g.
  ≤3); dedup against existing skills.
- **Cost** → at most one capped LLM call per cluster, few clusters per run.
- **Safety** → P2 only writes a *disabled* skill file; it changes no behavior
  until the human enables it. (This keeps P2 firmly inside the safety model:
  human-in-the-loop, reversible, evidence-backed — see `README.md`.)

## 11. Test plan

- Unit: clustering/normalization + dedup-vs-existing + rejected-guard over
  hand-built task rows.
- Integration (storage-only): seed `task_runs`, run the proposer with the LLM
  draft stubbed, assert a disabled `proposed` skill dir is written.
- Manual (pre-release): in the running app, click "从我的使用习惯提议技能",
  preview a proposal, enable it, confirm it then influences a new chat.
